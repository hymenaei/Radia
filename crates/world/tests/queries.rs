use std::{
    collections::HashSet,
    num::{NonZeroU8, NonZeroU32},
    ptr::eq,
};

use base::UUID;
use math::{DVec3, Quat, Vec3};
use world::{
    Applied,
    ApplyOutcome::{Introduced, RegionAdopted, RegionPatched, Removed, Updated},
    AttachmentPoint, CoordinateFrame, Entity, Kind,
    Kind::{Avatar, Object},
    LinearMotion, LocalAddress, ObjectObservation,
    OrderEvidence::Ordered,
    Parent, ParentUpdate,
    ParentUpdate::{Local, Root},
    Pose, PoseState, PropertiesPatch, Region, RegionHandle, RegionInfo, UnresolvedReason, UpdateSource, World, WorldLimits, WorldPose,
    WorldTransform, WorldTransformState,
};

fn world_limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 2,
        max_entities: 3,
        max_entities_per_region: 2,
        max_tombstones_per_region: 2,
        max_tombstones: 4,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(4),
        max_journal_entries: 12,
        max_journal_changes: 24,
        max_region_name_bytes: 32,
        max_recovery_targets: 2,
    }
}

fn uuid(value: u128) -> UUID {
    UUID::from_u128(value)
}

fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), 0)
}

fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID is nonzero")
}

fn object(region: Region, order: u64, id: u128, local: u32, kind: Kind, parent: ParentUpdate) -> ObjectObservation {
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
    }
}

fn entity(outcome: Applied) -> Entity {
    let Introduced(entity) = outcome.outcome else { panic!("expected object introduction") };
    entity
}

#[test]
fn borrowed_queries_expose_current_state() {
    let first = Region::new(1);
    let second = Region::new(2);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(first).unwrap().outcome, RegionAdopted);
    assert_eq!(world.adopt_region(second).unwrap().outcome, RegionAdopted);
    let first_info = RegionInfo {
        region_id: Some(uuid(10)),
        handle: Some(RegionHandle::new((256_u64 << 32) | 512)),
        name: Some("First".to_owned()),
        origin: None,
    };
    assert_eq!(world.patch_region(first, first_info.clone()).unwrap().outcome, RegionPatched);
    assert_eq!(world.region(first), Some(&first_info));
    assert_eq!(first.get(), 1);
    assert_eq!(first_info.handle.unwrap().get(), (256_u64 << 32) | 512);
    assert_eq!(first_info.handle.unwrap().origin(), DVec3::new(256.0, 512.0, 0.0));
    assert_eq!(world.region(Region::new(99)), None);

    let mut root = object(first, 1, 20, 1, Object, Root);
    root.pose = Some(Pose { position: Some(Vec3::new(3.0, 4.0, 5.0)), rotation: Some(Quat::IDENTITY) });
    root.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    root.angular_velocity = Some(Vec3::Z);
    root.scale = Some(Vec3::splat(2.0));
    root.attachment_point = Some(NonZeroU8::new(7).map(AttachmentPoint::new));
    let root = entity(world.observe_object(root).unwrap());
    let child = entity(world.observe_object(object(first, 2, 21, 2, Object, Local(local_id(1)))).unwrap());
    let avatar = entity(world.observe_object(object(second, 1, 22, 1, Avatar, Root)).unwrap());

    assert_eq!(world.entity(uuid(20)), Some(root));
    assert_eq!(world.entity(uuid(99)), None);
    assert_eq!(world.entity_at(LocalAddress::new(first, local_id(1))), Some(root));
    assert_eq!(world.object_count(), 3);
    assert!(world.revision().get() > 0);
    assert_eq!(world.objects().count(), 3);
    assert_eq!(world.objects_in_region(first).count(), 2);
    assert_eq!(world.objects_in_region(second).count(), 1);
    assert_eq!(world.regions().count(), 2);
    assert_eq!(world.children(root).count(), 1);
    assert_eq!(world.avatars().map(|object| object.entity()).collect::<Vec<_>>(), vec![avatar]);

    let view = world.object(root).expect("root exists");
    assert_eq!(view.entity(), root);
    assert_eq!(view.identity().id(), uuid(20));
    assert_eq!(view.kind(), Object);
    assert_eq!(view.address(), LocalAddress::new(first, local_id(1)));
    assert_eq!(view.address().local_id(), local_id(1));
    assert_eq!(view.parent(), Parent::Root);
    assert_eq!(view.position(), Some(Vec3::new(3.0, 4.0, 5.0)));
    assert_eq!(view.rotation(), Some(Quat::IDENTITY));
    assert_eq!(view.scale(), Some(Vec3::splat(2.0)));
    assert_eq!(view.local_transform().unwrap().scale, Vec3::splat(2.0));
    assert_eq!(view.linear_motion(), LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    assert_eq!(view.angular_velocity(), Some(Vec3::Z));
    assert_eq!(view.shape(), None);
    assert_eq!(view.appearance(), None);
    assert_eq!(view.attachment_point(), NonZeroU8::new(7).map(AttachmentPoint::new));
    assert_eq!(view.attachment_point().unwrap().get(), 7);
    assert_eq!(view.sit_target(), None);
    assert_eq!(view.properties().owner_id, None);
    assert_eq!(view.coordinate_frame().unwrap(), CoordinateFrame::RegionRelative { region: first });
    assert_eq!(
        view.world_transform().unwrap(),
        WorldTransformState::Resolved(WorldTransform {
            pose: WorldPose { position: DVec3::new(259.0, 516.0, 5.0), rotation: Quat::IDENTITY },
            scale: Vec3::splat(2.0),
        })
    );
    assert_eq!(world.object(child).unwrap().coordinate_frame().unwrap(), CoordinateFrame::LinksetRoot { root });
    assert_eq!(world.object(child).unwrap().world_pose().unwrap(), PoseState::Unresolved(UnresolvedReason::MissingLocalPose));
    drop(view);

    assert_eq!(
        world
            .patch_properties(source(first, 3), uuid(20), PropertiesPatch { owner_id: Some(uuid(30)), creator_id: Some(uuid(31)) })
            .unwrap()
            .outcome,
        Updated(root)
    );
    let properties = world.object(root).unwrap().properties();
    assert_eq!(properties.owner_id, Some(uuid(30)));
    assert_eq!(properties.creator_id, Some(uuid(31)));
}

#[test]
fn objects_returns_live_entities() {
    let region = Region::new(3);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);

    let obj = entity(world.observe_object(object(region, 1, 40, 1, Object, Root)).unwrap());
    let avatar = entity(world.observe_object(object(region, 2, 41, 2, Avatar, Root)).unwrap());
    assert_eq!(world.objects().map(|object| object.identity().id()).collect::<HashSet<_>>(), HashSet::from([uuid(40), uuid(41)]));
    assert_eq!(world.objects().count(), world.object_count());

    assert_eq!(world.remove_object(source(region, 3), local_id(2)).unwrap().outcome, Removed(uuid(41)));
    assert!(world.object(avatar).is_none());
    assert_eq!(world.objects().map(|object| object.identity().id()).collect::<HashSet<_>>(), HashSet::from([uuid(40)]));

    let replacement = entity(world.observe_object(object(region, 4, 41, 2, Avatar, Root)).unwrap());
    assert_ne!(avatar, replacement);
    assert_eq!(world.objects().map(|object| object.identity().id()).collect::<HashSet<_>>(), HashSet::from([uuid(40), uuid(41)]));
    assert_eq!(world.objects().count(), world.object_count());
    assert_eq!(world.object(obj).unwrap().identity().id(), uuid(40));
}

#[test]
fn object_views_borrow_the_same_identity() {
    let region = Region::new(4);
    let mut world = World::new(world_limits());
    assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
    let entity = entity(world.observe_object(object(region, 1, 50, 1, Object, Root)).unwrap());

    let first = world.object(entity).expect("object exists");
    let second = world.object(entity).expect("object exists");

    assert!(eq(first.identity(), second.identity()));
    assert_eq!(first.identity().id(), uuid(50));
    assert_eq!(second.kind(), Object);
    assert_eq!(first.address(), LocalAddress::new(region, local_id(1)));
}
