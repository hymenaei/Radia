use std::num::{NonZeroU8, NonZeroU32};

use base::UUID;
use math::{DVec3, Quat, Vec3};
use world::{
    Applied,
    ApplyOutcome::{Introduced, RegionAdopted, RegionPatched, Removed, Unchanged, Updated},
    AttachmentPoint, CoordinateFrame, Entity, FieldGroup, FieldRejection, FieldRejectionReason, Kind,
    Kind::{Avatar, Object},
    LocalAddress, ObjectObservation, ObjectPatch,
    OrderEvidence::Ordered,
    Parent, ParentUpdate,
    ParentUpdate::{Local, Root},
    Pose, PoseError, PoseState, Region, RegionHandle, RegionInfo, SitTarget, UnresolvedReason, UpdateAuthority, UpdateSource, World,
    WorldLimits, WorldPose, WorldTransformState,
};

fn limits(entities: usize) -> WorldLimits {
    WorldLimits {
        max_region_registrations: 2,
        max_entities: entities,
        max_entities_per_region: entities,
        max_tombstones_per_region: 4,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: entities.saturating_add(4),
        max_journal_changes: entities.saturating_mul(2).max(16),
        max_region_name_bytes: 64,
        max_recovery_targets: 8,
    }
}

fn uuid(value: u128) -> UUID {
    UUID::from_u128(value)
}

fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID is nonzero")
}

fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), 0)
}

fn address(region: Region, id: u32) -> LocalAddress {
    LocalAddress::new(region, local_id(id))
}

fn observation(region: Region, order: u64, id: u128, local: u32, kind: Kind, parent: ParentUpdate) -> ObjectObservation {
    ObjectObservation {
        source: source(region, order),
        id: uuid(id),
        local_id: local_id(local),
        kind,
        parent,
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: None,
        attachment_point: None,
        shape: None,
        appearance: None,
        surface_appearance: None,
        render_materials: None,
    }
}

fn with_pose(mut object: ObjectObservation, position: Vec3, rotation: Quat) -> ObjectObservation {
    object.pose = Some(Pose { position: Some(position), rotation: Some(rotation) });
    object
}

fn world_with_origin(region: Region, origin: DVec3, entities: usize) -> World {
    let mut world = World::new(limits(entities));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.patch_region(region, RegionInfo { origin: Some(origin), ..RegionInfo::default() }).unwrap().outcome, RegionPatched);
    world
}

fn introduced(outcome: Applied) -> Entity {
    let Introduced(entity) = outcome.outcome else { panic!("expected object introduction") };
    entity
}

#[test]
fn root_pose_uses_region_origin() {
    let region = Region::new(1);
    let origin = DVec3::new(1024.0, 2048.0, 0.0);
    let mut world = world_with_origin(region, origin, 4);
    let entity = introduced(
        world.observe_object(with_pose(observation(region, 1, 1, 1, Object, Root), Vec3::new(3.0, 4.0, 5.0), Quat::IDENTITY)).unwrap(),
    );

    assert_eq!(world.object(entity).unwrap().coordinate_frame().unwrap(), CoordinateFrame::RegionRelative { region });
    let PoseState::Resolved(pose) = world.object(entity).unwrap().world_pose().unwrap() else { panic!("expected resolved pose") };
    assert_eq!(pose.position, origin + DVec3::new(3.0, 4.0, 5.0));
    assert_eq!(pose.rotation, Quat::IDENTITY);
}

#[test]
fn nested_rotations_keep_f64_origin() {
    let region = Region::new(42);
    let origin = DVec3::new(2_f64.powi(45), -2_f64.powi(45), 4.0e12);
    let mut world = world_with_origin(region, origin, 4);
    let quarter_turn = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let half_turn = Quat::from_rotation_z(std::f32::consts::PI);
    let root = with_pose(observation(region, 1, 45, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), quarter_turn);
    let root = introduced(world.observe_object(root).unwrap());
    let child = with_pose(observation(region, 2, 46, 2, Object, Local(local_id(1))), Vec3::new(2.0, 0.0, 0.0), quarter_turn);
    let child = introduced(world.observe_object(child).unwrap());
    let grandchild = with_pose(observation(region, 3, 47, 3, Object, Local(local_id(2))), Vec3::new(1.0, 0.0, 0.0), Quat::IDENTITY);
    let grandchild = introduced(world.observe_object(grandchild).unwrap());

    let PoseState::Resolved(pose) = world.object(grandchild).unwrap().world_pose().unwrap() else { panic!("expected resolved pose") };
    assert!((pose.position.x - (origin.x + 9.0)).abs() < 0.01);
    assert_eq!(pose.position.y, origin.y + 2.0);
    assert_eq!(pose.position.z, origin.z);
    assert!(pose.rotation.dot(half_turn).abs() > 0.99999);
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(root));
}

#[test]
fn reused_parent_invalidates_pose() {
    let region = Region::new(45);
    let mut world = World::new(limits(3));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let old_parent = with_pose(observation(region, 10, 190, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY);
    introduced(world.observe_object(old_parent).unwrap());
    assert_eq!(world.remove_object(source(region, 12), local_id(1)).unwrap().outcome, Removed(uuid(190)));

    let child_pose = with_pose(observation(region, 15, 192, 2, Object, Local(local_id(1))), Vec3::X, Quat::IDENTITY);
    let child = introduced(world.observe_object(child_pose).unwrap());
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));

    let replacement = with_pose(observation(region, 20, 191, 1, Object, Root), Vec3::new(20.0, 0.0, 0.0), Quat::IDENTITY);
    let replacement = introduced(world.observe_object(replacement).unwrap());
    assert_eq!(world.object(child).unwrap().parent(), Parent::Pending { address: address(region, 1), expected_uuid: None });

    let mut refresh = ObjectPatch::new(source(region, 22), local_id(2));
    refresh.parent = Some(Local(local_id(1)));
    assert_eq!(world.patch_object(refresh).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(replacement));
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));
}

#[test]
fn resolved_pose_can_lack_scale() {
    let region = Region::new(14);
    let mut world = world_with_origin(region, DVec3::ZERO, 1);
    let object = with_pose(observation(region, 1, 17, 1, Object, Root), Vec3::X, Quat::IDENTITY);
    let entity = introduced(world.observe_object(object).unwrap());
    let pose = WorldPose { position: DVec3::X, rotation: Quat::IDENTITY };

    assert_eq!(world.object(entity).unwrap().world_pose().unwrap(), PoseState::Resolved(pose));
    assert_eq!(world.object(entity).unwrap().world_transform().unwrap(), WorldTransformState::MissingScale(pose));
}

#[test]
fn late_origin_resolves_existing_pose() {
    let region = Region::new(12);
    let mut world = World::new(limits(2));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let entity = introduced(world.observe_object(with_pose(observation(region, 1, 16, 1, Object, Root), Vec3::X, Quat::IDENTITY)).unwrap());
    assert_eq!(world.object(entity).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingRegionOrigin));

    assert_eq!(
        world.patch_region(region, RegionInfo { origin: Some(DVec3::new(20.0, 30.0, 0.0)), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );
    let PoseState::Resolved(pose) = world.object(entity).unwrap().world_pose().unwrap() else { panic!("expected resolved pose") };
    assert_eq!(pose.position, DVec3::new(21.0, 30.0, 0.0));
}

#[test]
fn handle_origin_resolves_pose() {
    let region = Region::new(15);
    let handle = RegionHandle::new((1024_u64 << 32) | 2048);
    let mut world = World::new(limits(1));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    assert_eq!(world.patch_region(region, RegionInfo { handle: Some(handle), ..RegionInfo::default() }).unwrap().outcome, RegionPatched);
    let entity = introduced(
        world.observe_object(with_pose(observation(region, 1, 18, 1, Object, Root), Vec3::new(3.0, 4.0, 5.0), Quat::IDENTITY)).unwrap(),
    );

    let PoseState::Resolved(pose) = world.object(entity).unwrap().world_pose().unwrap() else { panic!("expected handle-derived pose") };
    assert_eq!(pose.position, DVec3::new(1027.0, 2052.0, 5.0));
}

#[test]
fn pending_parent_is_first() {
    let region = Region::new(16);
    let mut world = World::new(limits(1));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let child = introduced(world.observe_object(observation(region, 1, 19, 1, Object, Local(local_id(2)))).unwrap());

    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));
}

#[test]
fn linkset_rotation_ignores_parent_scale() {
    let region = Region::new(2);
    let origin = DVec3::new(100.0, 200.0, 0.0);
    let mut world = world_with_origin(region, origin, 4);
    let parent_rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let parent = with_pose(observation(region, 1, 2, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), parent_rotation);
    let mut parent = parent;
    parent.scale = Some(Vec3::splat(4.0));
    let parent_entity = introduced(world.observe_object(parent).unwrap());
    let mut child = with_pose(observation(region, 2, 3, 2, Object, Local(local_id(1))), Vec3::new(2.0, 0.0, 0.0), Quat::IDENTITY);
    child.scale = Some(Vec3::splat(0.5));
    let child_entity = introduced(world.observe_object(child).unwrap());

    assert_eq!(world.object(child_entity).unwrap().coordinate_frame().unwrap(), CoordinateFrame::LinksetRoot { root: parent_entity });
    let PoseState::Resolved(pose) = world.object(child_entity).unwrap().world_pose().unwrap() else { panic!("expected resolved pose") };
    assert!((pose.position.x - 110.0).abs() < 1e-5);
    assert!((pose.position.y - 202.0).abs() < 1e-5);
    assert!(pose.rotation.dot(parent_rotation).abs() > 0.99999);
    let WorldTransformState::Resolved(transform) = world.object(child_entity).unwrap().world_transform().unwrap() else {
        panic!("expected resolved transform")
    };
    assert_eq!(transform.scale, Vec3::splat(0.5));
}

#[test]
fn parent_motion_updates_descendant_pose() {
    let region = Region::new(46);
    let mut world = world_with_origin(region, DVec3::ZERO, 3);
    let parent = with_pose(observation(region, 1, 52, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY);
    let parent = introduced(world.observe_object(parent).unwrap());
    let child = with_pose(observation(region, 2, 53, 2, Object, Local(local_id(1))), Vec3::new(2.0, 0.0, 0.0), Quat::IDENTITY);
    let child = introduced(world.observe_object(child).unwrap());
    let PoseState::Resolved(pose) = world.object(child).unwrap().world_pose().unwrap() else { panic!("expected initial pose") };
    assert_eq!(pose.position, DVec3::new(12.0, 0.0, 0.0));

    let mut move_parent = ObjectPatch::new(source(region, 3), local_id(1));
    move_parent.pose = Some(Pose { position: Some(Vec3::new(20.0, 0.0, 0.0)), rotation: None });
    assert_eq!(world.patch_object(move_parent).unwrap().outcome, Updated(parent));
    let PoseState::Resolved(pose) = world.object(child).unwrap().world_pose().unwrap() else { panic!("expected moved pose") };
    assert_eq!(pose.position, DVec3::new(22.0, 0.0, 0.0));
}

#[test]
fn avatar_transform_uses_accepted_scale() {
    let region = Region::new(47);
    let mut world = world_with_origin(region, DVec3::ZERO, 1);
    let scale = Vec3::new(0.7, 0.7, 1.8);
    let mut avatar = with_pose(observation(region, 1, 54, 1, Avatar, Root), Vec3::new(1.0, 2.0, 3.0), Quat::IDENTITY);
    avatar.scale = Some(scale);
    let avatar = introduced(world.observe_object(avatar).unwrap());

    let WorldTransformState::Resolved(transform) = world.object(avatar).unwrap().world_transform().unwrap() else {
        panic!("expected avatar transform")
    };
    assert_eq!(transform.scale, scale);
    assert_eq!(transform.pose.position, DVec3::new(1.0, 2.0, 3.0));
}

#[test]
fn sit_target_does_not_change_pose() {
    let region = Region::new(43);
    let mut world = world_with_origin(region, DVec3::new(100.0, 200.0, 0.0), 3);
    let seat = with_pose(
        observation(region, 1, 48, 1, Object, Root),
        Vec3::new(10.0, 0.0, 0.0),
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
    );
    introduced(world.observe_object(seat).unwrap());
    let avatar = with_pose(observation(region, 2, 49, 2, Avatar, Local(local_id(1))), Vec3::new(2.0, 0.0, 0.0), Quat::IDENTITY);
    let avatar = introduced(world.observe_object(avatar).unwrap());
    let before = world.object(avatar).unwrap().world_pose().unwrap();
    let PoseState::Resolved(before_pose) = before else { panic!("expected resolved pose") };
    assert!((before_pose.position.x - 110.0).abs() < 1e-5);
    assert!((before_pose.position.y - 202.0).abs() < 1e-5);
    assert!(before_pose.rotation.dot(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)).abs() > 0.99999);

    assert_eq!(
        world
            .patch_sit_target(
                source(region, 3),
                uuid(49),
                Some(SitTarget { position: Vec3::new(50.0, 60.0, 70.0), rotation: Quat::from_rotation_x(1.0) }),
            )
            .unwrap()
            .outcome,
        Updated(avatar)
    );
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), before);

    assert_eq!(world.patch_sit_target(source(region, 4), uuid(49), None).unwrap().outcome, Updated(avatar));
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), before);
}

#[test]
fn missing_local_pose_precedes_missing_origin() {
    let region = Region::new(44);
    let mut world = World::new(limits(2));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let no_pose = introduced(world.observe_object(observation(region, 1, 50, 1, Object, Root)).unwrap());
    assert_eq!(world.object(no_pose).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingLocalPose));

    let pose = with_pose(observation(region, 2, 51, 2, Object, Root), Vec3::X, Quat::IDENTITY);
    let missing_origin = introduced(world.observe_object(pose).unwrap());
    assert_eq!(world.object(missing_origin).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingRegionOrigin));
}

#[test]
fn missing_pose_beats_joint() {
    let region = Region::new(18);
    let mut world = World::new(limits(2));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    introduced(world.observe_object(observation(region, 1, 22, 1, Avatar, Root)).unwrap());
    let child = with_pose(observation(region, 2, 23, 2, Object, Local(local_id(1))), Vec3::X, Quat::IDENTITY);
    let child = introduced(world.observe_object(child).unwrap());

    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingLocalPose));
}

#[test]
fn pending_parent_resolution_keeps_pose() {
    let region = Region::new(3);
    let mut world = world_with_origin(region, DVec3::ZERO, 4);
    let child = with_pose(observation(region, 2, 5, 2, Object, Local(local_id(1))), Vec3::X, Quat::IDENTITY);
    let child_entity = introduced(world.observe_object(child).unwrap());
    assert_eq!(world.object(child_entity).unwrap().parent(), Parent::Pending { address: address(region, 1), expected_uuid: None });
    assert_eq!(world.object(child_entity).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));

    let parent = with_pose(observation(region, 1, 4, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY);
    let parent_entity = introduced(world.observe_object(parent).unwrap());

    assert_eq!(world.object(child_entity).unwrap().parent(), Parent::Resolved(parent_entity));
    let PoseState::Resolved(pose) = world.object(child_entity).unwrap().world_pose().unwrap() else { panic!("expected resolved pose") };
    assert_eq!(pose.position, DVec3::new(11.0, 0.0, 0.0));
}

#[test]
fn cascade_rebind_keeps_pose() {
    let region = Region::new(49);
    let mut world = world_with_origin(region, DVec3::ZERO, 3);
    let root = with_pose(observation(region, 1, 194, 1, Object, Root), Vec3::ZERO, Quat::IDENTITY);
    introduced(world.observe_object(root).unwrap());
    let parent = with_pose(observation(region, 2, 195, 2, Object, Local(local_id(1))), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY);
    introduced(world.observe_object(parent).unwrap());
    let child_pose = with_pose(observation(region, 3, 196, 3, Object, Local(local_id(2))), Vec3::X, Quat::IDENTITY);
    let child = introduced(world.observe_object(child_pose).unwrap());

    let rebind = observation(region, 4, 196, 1, Object, Local(local_id(2)));
    assert_eq!(world.observe_object(rebind).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().parent(), Parent::Pending { address: address(region, 2), expected_uuid: Some(uuid(195)) });
    assert_eq!(world.entity(uuid(195)), None);
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));

    let parent = with_pose(observation(region, 5, 195, 2, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY);
    let parent = introduced(world.observe_object(parent).unwrap());
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(parent));
    assert_eq!(
        world.object(child).unwrap().world_pose().unwrap(),
        PoseState::Resolved(WorldPose { position: DVec3::new(11.0, 0.0, 0.0), rotation: Quat::IDENTITY })
    );
}

#[test]
fn parent_change_requires_fresh_pose() {
    let region = Region::new(4);
    let mut world = world_with_origin(region, DVec3::ZERO, 4);
    let parent = introduced(
        world.observe_object(with_pose(observation(region, 1, 6, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let child = introduced(world.observe_object(with_pose(observation(region, 2, 7, 2, Object, Root), Vec3::X, Quat::IDENTITY)).unwrap());

    let mut reparent = ObjectPatch::new(source(region, 3), local_id(2));
    reparent.parent = Some(Local(local_id(1)));
    assert_eq!(world.patch_object(reparent).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));

    let mut pose = ObjectPatch::new(source(region, 4), local_id(2));
    pose.pose = Some(Pose { position: Some(Vec3::new(2.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    assert_eq!(world.patch_object(pose).unwrap().outcome, Updated(child));
    let PoseState::Resolved(pose) = world.object(child).unwrap().world_pose().unwrap() else { panic!("expected resolved pose") };
    assert_eq!(pose.position, DVec3::new(12.0, 0.0, 0.0));
    assert_eq!(world.children(parent).count(), 1);
}

#[test]
fn repeated_parent_update_is_not_a_cycle() {
    let region = Region::new(65);
    let mut world = world_with_origin(region, DVec3::ZERO, 3);
    let parent = introduced(world.observe_object(observation(region, 1, 650, 1, Object, Root)).unwrap());
    let child = introduced(world.observe_object(observation(region, 2, 651, 2, Object, Local(local_id(1)))).unwrap());

    let mut update = ObjectPatch::new(source(region, 3), local_id(2));
    update.parent = Some(Local(local_id(1)));
    let applied = world.patch_object(update).unwrap();

    assert_eq!(applied.outcome, Unchanged);
    assert!(applied.rejections.is_empty());
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(parent));
}

#[test]
fn delayed_observation_cannot_apply_pose_from_an_old_parent_frame() {
    let region = Region::new(64);
    let mut world = world_with_origin(region, DVec3::ZERO, 5);
    let first_parent = introduced(
        world.observe_object(with_pose(observation(region, 1, 640, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let second_parent = introduced(
        world.observe_object(with_pose(observation(region, 2, 641, 2, Object, Root), Vec3::new(200.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let child = introduced(world.observe_object(with_pose(observation(region, 3, 642, 3, Object, Root), Vec3::X, Quat::IDENTITY)).unwrap());

    let mut reparent = ObjectPatch::new(source(region, 20), local_id(3));
    reparent.parent = Some(Local(local_id(2)));
    assert_eq!(world.patch_object(reparent).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));

    let delayed = with_pose(observation(region, 15, 642, 3, Object, Local(local_id(1))), Vec3::new(5.0, 0.0, 0.0), Quat::IDENTITY);
    let applied = world.observe_object(delayed).unwrap();
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::StaleFrame }]
    );
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(second_parent));
    assert_eq!(world.object(child).unwrap().position(), Some(Vec3::X));
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));

    let mut same_update = ObjectPatch::new(source(region, 21), local_id(3));
    same_update.parent = Some(Local(local_id(1)));
    same_update.pose = Some(Pose { position: Some(Vec3::new(2.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    let applied = world.patch_object(same_update).unwrap();
    assert!(applied.rejections.is_empty());
    let PoseState::Resolved(pose) = world.object(child).unwrap().world_pose().unwrap() else {
        panic!("same-update pose must use the accepted parent frame")
    };
    assert_eq!(pose.position, DVec3::new(12.0, 0.0, 0.0));
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(first_parent));
}

#[test]
fn partial_pose_frames_update_independently() {
    let region = Region::new(40);
    let mut world = world_with_origin(region, DVec3::ZERO, 4);
    let first_parent = introduced(
        world.observe_object(with_pose(observation(region, 1, 40, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let mut child_observation = observation(region, 2, 41, 2, Object, Local(local_id(1)));
    child_observation.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    let child = introduced(world.observe_object(child_observation).unwrap());
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingLocalPose));

    let mut rotation = ObjectPatch::new(source(region, 3), local_id(2));
    rotation.pose = Some(Pose { position: None, rotation: Some(Quat::IDENTITY) });
    assert_eq!(world.patch_object(rotation).unwrap().outcome, Updated(child));
    let PoseState::Resolved(pose) = world.object(child).unwrap().world_pose().unwrap() else { panic!("expected initial pose") };
    assert_eq!(pose.position, DVec3::new(11.0, 0.0, 0.0));

    let second_parent = introduced(
        world.observe_object(with_pose(observation(region, 4, 42, 3, Object, Root), Vec3::new(20.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let mut reparent = ObjectPatch::new(source(region, 5), local_id(2));
    reparent.parent = Some(Local(local_id(3)));
    assert_eq!(world.patch_object(reparent).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));

    for pose in [Pose { position: Some(Vec3::Y), rotation: None }, Pose { position: None, rotation: Some(Quat::from_rotation_z(0.5)) }] {
        let mut delayed_pose = ObjectPatch::new(source(region, 4), local_id(2));
        delayed_pose.pose = Some(pose);
        let applied = world.patch_object(delayed_pose).unwrap();
        assert_eq!(
            applied.rejections.iter().copied().collect::<Vec<_>>(),
            [FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::StaleFrame }]
        );
        assert_eq!(world.object(child).unwrap().position(), Some(Vec3::X));
    }

    let mut position = ObjectPatch::new(source(region, 6), local_id(2));
    position.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    assert_eq!(world.patch_object(position).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));

    let mut rotation = ObjectPatch::new(source(region, 7), local_id(2));
    rotation.pose = Some(Pose { position: None, rotation: Some(Quat::IDENTITY) });
    assert_eq!(world.patch_object(rotation).unwrap().outcome, Updated(child));
    let PoseState::Resolved(pose) = world.object(child).unwrap().world_pose().unwrap() else { panic!("expected reparented pose") };
    assert_eq!(pose.position, DVec3::new(21.0, 0.0, 0.0));
    assert_eq!(world.object(child).unwrap().parent(), Parent::Resolved(second_parent));
    assert_ne!(first_parent, second_parent);
}

#[test]
fn partial_pose_replaces_detach_snapshot() {
    let region = Region::new(41);
    let mut world = world_with_origin(region, DVec3::new(100.0, 0.0, 0.0), 3);
    let prim = introduced(
        world.observe_object(with_pose(observation(region, 1, 43, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let avatar = introduced(
        world.observe_object(with_pose(observation(region, 2, 44, 2, Avatar, Local(local_id(1))), Vec3::X, Quat::IDENTITY)).unwrap(),
    );

    let mut detach = ObjectPatch::new(source(region, 3), local_id(2));
    detach.parent = Some(Root);
    assert_eq!(world.patch_object(detach).unwrap().outcome, Updated(avatar));
    assert!(matches!(world.object(avatar).unwrap().world_pose().unwrap(), PoseState::Provisional { .. }));

    let mut position = ObjectPatch::new(source(region, 4), local_id(2));
    position.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    assert_eq!(world.patch_object(position).unwrap().outcome, Updated(avatar));
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));

    let mut rotation = ObjectPatch::new(source(region, 5), local_id(2));
    rotation.pose = Some(Pose { position: None, rotation: Some(Quat::IDENTITY) });
    assert_eq!(world.patch_object(rotation).unwrap().outcome, Updated(avatar));
    let PoseState::Resolved(pose) = world.object(avatar).unwrap().world_pose().unwrap() else { panic!("expected detached pose") };
    assert_eq!(pose.position, DVec3::new(101.0, 0.0, 0.0));
    assert_eq!(world.object(avatar).unwrap().parent(), Parent::Root);
    assert_eq!(world.object(prim).unwrap().kind(), Object);
}

#[test]
fn removed_seat_keeps_avatar_pending() {
    let region = Region::new(42);
    let mut world = world_with_origin(region, DVec3::new(100.0, 0.0, 0.0), 3);
    let seat = introduced(
        world.observe_object(with_pose(observation(region, 1, 45, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let avatar = introduced(
        world.observe_object(with_pose(observation(region, 2, 46, 2, Avatar, Local(local_id(1))), Vec3::X, Quat::IDENTITY)).unwrap(),
    );
    assert_eq!(world.remove_object(source(region, 3), local_id(1)).unwrap().outcome, Removed(uuid(45)));

    assert!(world.object(seat).is_none());
    let avatar = world.object(avatar).unwrap();
    assert_eq!(avatar.parent(), Parent::Pending { address: address(region, 1), expected_uuid: Some(uuid(45)) });
    assert_eq!(avatar.world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));
}

#[test]
fn seat_kill_requires_fresh_unseat() {
    let region = Region::new(42);
    let mut world = world_with_origin(region, DVec3::new(100.0, 0.0, 0.0), 3);
    let seat = introduced(
        world.observe_object(with_pose(observation(region, 1, 45, 1, Object, Root), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let avatar = introduced(
        world.observe_object(with_pose(observation(region, 2, 46, 2, Avatar, Local(local_id(1))), Vec3::X, Quat::IDENTITY)).unwrap(),
    );
    assert_eq!(world.remove_object(source(region, 3), local_id(1)).unwrap().outcome, Removed(uuid(45)));
    assert!(world.object(seat).is_none());

    let mut stale_unseat = ObjectPatch::new(source(region, 3), local_id(2));
    stale_unseat.parent = Some(Root);
    assert_eq!(world.patch_object(stale_unseat).unwrap().outcome, Unchanged);
    assert_eq!(world.object(avatar).unwrap().parent(), Parent::Pending { address: address(region, 1), expected_uuid: Some(uuid(45)) });
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));

    let untracked_source = source(region, 100).with_authority(UpdateAuthority::Untracked);
    let mut untracked_unseat = ObjectPatch::new(untracked_source, local_id(2));
    untracked_unseat.parent = Some(Root);
    let untracked = world.patch_object(untracked_unseat).unwrap();
    assert_eq!(untracked.outcome, Unchanged);
    assert!(
        untracked
            .rejections
            .iter()
            .any(|rejection| { rejection.group == FieldGroup::Parent && rejection.reason == FieldRejectionReason::UntrustedSource })
    );
    assert_eq!(world.object(avatar).unwrap().parent(), Parent::Pending { address: address(region, 1), expected_uuid: Some(uuid(45)) });
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PendingParent));

    let mut fresh_unseat = ObjectPatch::new(source(region, 4), local_id(2));
    fresh_unseat.parent = Some(Root);
    assert_eq!(world.patch_object(fresh_unseat).unwrap().outcome, Updated(avatar));
    assert_eq!(world.object(avatar).unwrap().parent(), Parent::Root);
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));
}

#[test]
fn transfer_accepts_destination_pose() {
    let first = Region::new(5);
    let second = Region::new(6);
    let mut world = World::new(limits(4));
    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    assert_eq!(
        world.patch_region(first, RegionInfo { origin: Some(DVec3::new(100.0, 0.0, 0.0)), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );
    assert_eq!(
        world.patch_region(second, RegionInfo { origin: Some(DVec3::new(900.0, 0.0, 0.0)), ..RegionInfo::default() }).unwrap().outcome,
        RegionPatched
    );
    let entity = introduced(world.observe_object(with_pose(observation(first, 100, 8, 1, Object, Root), Vec3::X, Quat::IDENTITY)).unwrap());

    let destination = with_pose(observation(second, 1, 8, 1, Object, Root), Vec3::new(3.0, 0.0, 0.0), Quat::IDENTITY);
    assert_eq!(world.observe_object(destination).unwrap().outcome, Updated(entity));

    assert_eq!(world.object(entity).unwrap().address(), address(second, 1));
    let PoseState::Resolved(pose) = world.object(entity).unwrap().world_pose().unwrap() else {
        panic!("expected resolved destination pose")
    };
    assert_eq!(pose.position, DVec3::new(903.0, 0.0, 0.0));
}

#[test]
fn attachment_waits_for_joint_pose() {
    let region = Region::new(7);
    let mut world = World::new(limits(4));
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let avatar =
        introduced(world.observe_object(with_pose(observation(region, 1, 9, 1, Avatar, Root), Vec3::ZERO, Quat::IDENTITY)).unwrap());
    let attachment = with_pose(observation(region, 2, 10, 2, Object, Local(local_id(1))), Vec3::X, Quat::IDENTITY);
    let attachment = introduced(world.observe_object(attachment).unwrap());

    assert_eq!(world.object(attachment).unwrap().coordinate_frame().unwrap(), CoordinateFrame::AttachmentPoint { avatar, point: None });
    assert_eq!(world.object(attachment).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingAttachmentJoint));
}

#[test]
fn attachment_change_stales_pose() {
    let region = Region::new(17);
    let mut world = world_with_origin(region, DVec3::ZERO, 4);
    introduced(world.observe_object(with_pose(observation(region, 1, 20, 1, Avatar, Root), Vec3::ZERO, Quat::IDENTITY)).unwrap());
    let mut initial = with_pose(observation(region, 2, 21, 2, Object, Local(local_id(1))), Vec3::X, Quat::IDENTITY);
    initial.attachment_point = Some(NonZeroU8::new(3).map(AttachmentPoint::new));
    let attachment = introduced(world.observe_object(initial).unwrap());
    assert_eq!(world.object(attachment).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingAttachmentJoint));

    let mut change = ObjectPatch::new(source(region, 4), local_id(2));
    change.attachment_point = Some(NonZeroU8::new(4).map(AttachmentPoint::new));
    assert_eq!(world.patch_object(change).unwrap().outcome, Updated(attachment));
    assert_eq!(
        world.object(attachment).unwrap().world_pose().unwrap(),
        PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame)
    );

    let mut delayed_pose = ObjectPatch::new(source(region, 3), local_id(2));
    delayed_pose.pose = Some(Pose { position: Some(Vec3::new(9.0, 0.0, 0.0)), rotation: None });
    let applied = world.patch_object(delayed_pose).unwrap();
    assert_eq!(
        applied.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::StaleFrame }]
    );
    assert_eq!(world.object(attachment).unwrap().position(), Some(Vec3::X));

    let mut fresh_pose = ObjectPatch::new(source(region, 5), local_id(2));
    fresh_pose.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) });
    assert_eq!(world.patch_object(fresh_pose).unwrap().outcome, Updated(attachment));
    assert_eq!(world.object(attachment).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingAttachmentJoint));

    let mut clear = ObjectPatch::new(source(region, 6), local_id(2));
    clear.attachment_point = Some(None);
    assert_eq!(world.patch_object(clear).unwrap().outcome, Updated(attachment));
    assert_eq!(
        world.object(attachment).unwrap().world_pose().unwrap(),
        PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame)
    );
}

#[test]
fn same_attachment_point_keeps_pose_frame() {
    let region = Region::new(12);
    let mut world = world_with_origin(region, DVec3::ZERO, 4);
    let avatar =
        introduced(world.observe_object(with_pose(observation(region, 1, 15, 1, Avatar, Root), Vec3::ZERO, Quat::IDENTITY)).unwrap());
    let point = NonZeroU8::new(3).map(AttachmentPoint::new);
    let mut attached = with_pose(observation(region, 2, 16, 2, Object, Local(local_id(1))), Vec3::X, Quat::IDENTITY);
    attached.attachment_point = Some(point);
    let attachment = introduced(world.observe_object(attached).unwrap());
    let before = world.object(attachment).unwrap().world_pose().unwrap();
    assert_eq!(before, PoseState::Unresolved(UnresolvedReason::MissingAttachmentJoint));
    assert_eq!(world.object(attachment).unwrap().coordinate_frame().unwrap(), CoordinateFrame::AttachmentPoint { avatar, point });

    let mut repeated = observation(region, 3, 16, 2, Object, Local(local_id(1)));
    repeated.attachment_point = Some(point);
    assert_eq!(world.observe_object(repeated).unwrap().outcome, Unchanged);

    assert_eq!(world.object(attachment).unwrap().world_pose().unwrap(), before);
}

#[test]
fn detach_keeps_only_resolved_snapshot() {
    let region = Region::new(8);
    let origin = DVec3::new(50.0, 60.0, 0.0);
    let mut world = world_with_origin(region, origin, 4);
    let _seat = introduced(
        world.observe_object(with_pose(observation(region, 1, 11, 1, Object, Root), Vec3::new(4.0, 5.0, 0.0), Quat::IDENTITY)).unwrap(),
    );
    let avatar = introduced(
        world
            .observe_object(with_pose(observation(region, 2, 12, 2, Avatar, Local(local_id(1))), Vec3::new(1.0, 2.0, 0.0), Quat::IDENTITY))
            .unwrap(),
    );

    let mut detach = ObjectPatch::new(source(region, 3), local_id(2));
    detach.parent = Some(Root);
    assert_eq!(world.patch_object(detach).unwrap().outcome, Updated(avatar));
    let PoseState::Provisional { pose, reason } = world.object(avatar).unwrap().world_pose().unwrap() else {
        panic!("expected pre-detach pose snapshot")
    };
    assert_eq!(pose.position, origin + DVec3::new(5.0, 7.0, 0.0));
    assert_eq!(reason, UnresolvedReason::PoseNotValidForCurrentFrame);

    let missing = Region::new(9);
    let mut world = World::new(limits(4));
    assert_eq!(world.adopt_region(missing).unwrap().outcome, RegionAdopted);
    let _seat =
        introduced(world.observe_object(with_pose(observation(missing, 1, 13, 1, Object, Root), Vec3::ZERO, Quat::IDENTITY)).unwrap());
    let avatar = introduced(
        world.observe_object(with_pose(observation(missing, 2, 14, 2, Avatar, Local(local_id(1))), Vec3::X, Quat::IDENTITY)).unwrap(),
    );
    let mut detach = ObjectPatch::new(source(missing, 3), local_id(2));
    detach.parent = Some(Root);
    assert_eq!(world.patch_object(detach).unwrap().outcome, Updated(avatar));
    assert_eq!(world.object(avatar).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));
}

#[test]
fn transfer_clears_region_sit_target() {
    let first = Region::new(10);
    let second = Region::new(11);
    let mut world = World::new(limits(4));
    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    let entity =
        introduced(world.observe_object(with_pose(observation(first, 10, 15, 1, Avatar, Root), Vec3::ZERO, Quat::IDENTITY)).unwrap());
    assert_eq!(
        world
            .patch_sit_target(source(first, 11), uuid(15), Some(SitTarget { position: Vec3::X, rotation: Quat::IDENTITY }))
            .unwrap()
            .outcome,
        Updated(entity)
    );
    assert!(world.object(entity).unwrap().sit_target().is_some());

    let destination = with_pose(observation(second, 1, 15, 1, Avatar, Root), Vec3::ZERO, Quat::IDENTITY);
    assert_eq!(world.observe_object(destination).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().sit_target(), None);
}

#[test]
fn parent_depth_limit_is_enforced() {
    let region = Region::new(13);
    let mut world = world_with_origin(region, DVec3::ZERO, 18);
    let mut last = None;
    let mut root = None;
    for index in 0..=16 {
        let parent = if index == 0 { Root } else { Local(local_id(index)) };
        let entity = introduced(
            world
                .observe_object(with_pose(
                    observation(region, u64::from(index) + 1, u128::from(index) + 20, index + 1, Object, parent),
                    Vec3::X,
                    Quat::IDENTITY,
                ))
                .unwrap(),
        );
        if index == 0 {
            root = Some(entity);
        }
        last = Some(entity);
    }
    let last = last.expect("last entity");
    assert!(matches!(world.object(last).unwrap().world_pose(), Ok(PoseState::Resolved(_))));
    assert_eq!(world.object(last).unwrap().coordinate_frame(), Ok(CoordinateFrame::LinksetRoot { root: root.unwrap() }));

    let last = introduced(
        world.observe_object(with_pose(observation(region, 18, 37, 18, Object, Local(local_id(17))), Vec3::X, Quat::IDENTITY)).unwrap(),
    );
    assert_eq!(world.object(last).unwrap().world_pose(), Err(PoseError::TraversalLimitExceeded { max_edges: 16 }));
    assert_eq!(world.object(last).unwrap().coordinate_frame(), Err(PoseError::TraversalLimitExceeded { max_edges: 16 }));
}
