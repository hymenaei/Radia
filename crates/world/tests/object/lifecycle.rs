use super::support::*;

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
fn live_binding_precedes_region_floor() {
    let region = Region::new(43);
    let mut world_limits = limits();
    world_limits.max_tombstones_per_region = 1;
    world_limits.max_tombstones = 1;
    world_limits.max_tombstone_bytes_per_region = WorldLimits::tombstone_bytes_for_count(1);
    world_limits.max_tombstone_bytes = WorldLimits::tombstone_bytes_for_count(1);
    let mut world = World::new(world_limits);
    assert!(matches!(world.adopt_region(region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
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
fn local_ids_are_region_scoped() {
    let first_region = Region::new(22);
    let second_region = Region::new(23);
    let mut world = World::new(limits());
    assert!(matches!(world.adopt_region(first_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    assert!(matches!(world.adopt_region(second_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));

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
fn world_queries_follow_lifecycle() {
    let first_region = Region::new(20);
    let second_region = Region::new(21);
    let mut world = World::new(limits());
    assert!(matches!(world.adopt_region(first_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    assert!(matches!(world.adopt_region(second_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    assert!(matches!(
        world.patch_region(first_region, RegionInfo { name: Some("First".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
        world::RegionOutcome::Patched { .. }
    ));
    assert!(matches!(
        world.patch_region(second_region, RegionInfo { name: Some("Second".to_owned()), ..RegionInfo::default() }).unwrap().outcome,
        world::RegionOutcome::Patched { .. }
    ));

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
    assert_eq!(world.objects().iter().count(), 4);
    assert_eq!(world.objects_in_region(first_region).map(|object| object.id()).collect::<HashSet<_>>(), [uuid(200), uuid(201)].into());
    assert_eq!(world.objects_in_region(second_region).map(|object| object.id()).collect::<HashSet<_>>(), [uuid(202), uuid(203)].into());
    assert_eq!(world.objects_in_region(Region::new(99)).count(), 0);
    assert_eq!(world.children(parent_entity).map(|object| object.id()).collect::<HashSet<_>>(), [uuid(201)].into());
    assert_eq!(world.children(child_entity).count(), 0);
    assert_eq!(world.avatars().map(|object| object.id()).collect::<HashSet<_>>(), [uuid(202)].into());
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
    assert_eq!(world.objects_in_region(second_region).map(|object| object.id()).collect::<HashSet<_>>(), [uuid(202)].into());
    assert_eq!(world.avatars().map(|object| object.id()).collect::<HashSet<_>>(), [uuid(202)].into());

    let replacement = world.observe_object(object(source(second_region, 4), 203, 4, Object, Root)).unwrap();
    let Introduced(replacement_entity) = replacement.outcome else { panic!("expected replacement introduction") };
    assert_ne!(other_entity, replacement_entity);
    assert!(world.object(other_entity).is_none());
    assert_eq!(world.entity(uuid(203)), Some(replacement_entity));
    assert_eq!(world.object_count(), 4);
    assert_eq!(world.objects_in_region(second_region).map(|object| object.id()).collect::<HashSet<_>>(), [uuid(202), uuid(203)].into());
    assert_eq!(world.object(avatar_entity).unwrap().id(), uuid(202));
}

#[test]
fn avatar_queries_follow_rebinding_replacement_and_retirement() {
    let first = Region::new(1);
    let second = Region::new(2);
    let mut world = with_region(first);
    let _ = world.adopt_region(second).unwrap();
    let Introduced(avatar) = world.observe_object(object(source(first, 1), 10, 1, Avatar, Root)).unwrap().outcome else {
        panic!("expected avatar introduction")
    };
    let _ = world.observe_object(object(source(first, 2), 10, 2, Avatar, Root)).unwrap();
    let _ = world.observe_object(object(source(second, 1), 10, 1, Avatar, Root)).unwrap();
    assert_eq!(world.avatars().map(|object| (object.entity(), object.address())).collect::<Vec<_>>(), [(avatar, address(second, 1))]);

    assert_eq!(
        world.observe_object(object(source(second, 2), 10, 1, Object, Root)),
        Err(KindMismatch { current: Avatar, observed: Object })
    );
    assert_eq!(world.avatars().map(|object| object.entity()).collect::<Vec<_>>(), [avatar]);

    let _ = world.observe_object(object(source(second, 3), 11, 1, Object, Root)).unwrap();
    assert!(world.object(avatar).is_none());
    assert_eq!(world.avatars().count(), 0);
    let Introduced(replacement) = world.observe_object(object(source(second, 4), 10, 2, Avatar, Root)).unwrap().outcome else {
        panic!("expected replacement avatar")
    };
    assert_ne!(replacement, avatar);
    assert_eq!(world.avatars().map(|object| object.entity()).collect::<Vec<_>>(), [replacement]);

    let _ = world.retire_region(second).unwrap();
    assert_eq!(world.avatars().count(), 0);
    #[cfg(feature = "invariants")]
    assert_eq!(world.check_invariants(), Ok(()));
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
    assert_eq!(applied.recovery.requests, vec![address(region, 272)]);
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
    assert_eq!(world.object(avatar).unwrap().parent(), Pending { address: address(region, 1), expected_uuid: Some(uuid(1)) });
    assert_eq!(world.object(target).unwrap().parent(), Resolved(avatar));
    assert_eq!(world.object(replacement).unwrap().parent(), Resolved(target));
}

#[test]
fn transfer_ignores_old_region_barrier() {
    let first_region = Region::new(70);
    let second_region = Region::new(71);
    let mut world = with_region(first_region);
    assert!(matches!(world.adopt_region(second_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));

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
fn rebind_fences_old_local_id_at_rebind_stamp() {
    let region = Region::new(13);
    let mut world = with_region(region);
    let created = world.observe_object(object(source(region, 100), 130, 1, Object, Root)).unwrap();
    let Introduced(original) = created.outcome else { panic!("expected introduction") };

    assert_eq!(world.observe_object(object(source(region, 150), 130, 2, Object, Root)).unwrap().outcome, Updated(original));
    assert_eq!(world.observe_object(object(source(region, 120), 131, 1, Object, Root)).unwrap().outcome, IgnoredStale);
    assert_eq!(world.entity(uuid(131)), None);

    let replacement = world.observe_object(object(source(region, 160), 131, 1, Object, Root)).unwrap();
    let Introduced(replacement) = replacement.outcome else { panic!("expected a new binding after the rebind tombstone") };
    assert_ne!(original, replacement);
    assert_eq!(world.entity(uuid(130)), Some(original));
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
    assert_eq!(applied.recovery.requests, [address(region, 7)]);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.entity(uuid(510)), Some(entity));
}

#[test]
fn recovery_targets_respect_world_limit() {
    let region = Region::new(41);
    let mut world_limits = limits();
    world_limits.max_recovery_targets = 2;
    let mut world = World::new(world_limits);
    assert!(matches!(world.adopt_region(region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));

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
    let targets = removed.recovery.requests;
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|target| [2, 3, 4].contains(&target.local_id().get())));
    assert!(
        child_entities
            .iter()
            .all(|child| world.object(*child).unwrap().parent() == Pending { address: address(region, 1), expected_uuid: Some(uuid(410)) })
    );
}

#[test]
fn agent_region_change_requires_handoff() {
    let source_region = Region::new(4);
    let destination_region = Region::new(5);
    let mut world = World::new(limits());
    assert!(matches!(world.adopt_region(source_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    assert!(matches!(world.adopt_region(destination_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
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
        patch_properties_current(
            &mut world,
            source(source_region, 101),
            agent_id,
            PropertiesPatch { owner_id: Some(uuid(41)), creator_id: None }
        )
        .unwrap()
        .outcome,
        Updated(_)
    ));
    assert_eq!(world.remove_object(source(source_region, 102), local_id(8)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(agent_id), Some(agent));
    let ambiguous_kill = UpdateSource::new(source_region, 103, Ambiguous, 0);
    let ignored = world.remove_object(ambiguous_kill, local_id(8)).unwrap();
    assert_eq!(ignored.outcome, Unchanged);
    assert!(ignored.recovery.requests.is_empty());
    assert_eq!(world.entity(agent_id), Some(agent));

    let direct_move = world.observe_object(object(source(destination_region, 1), 40, 8, Avatar, Root));
    assert_eq!(direct_move, Err(InvalidValue { field: "agent_handoff_required" }));
    assert_eq!(world.object(agent).unwrap().address(), address(source_region, 8));
    let other_avatar = world.handoff_agent(source_region, object(source(destination_region, 1), 41, 9, Avatar, Root));
    assert_eq!(other_avatar, Err(InvalidValue { field: "agent_id" }));
    assert_eq!(world.object(agent).unwrap().address(), address(source_region, 8));

    let applied = world.handoff_agent(source_region, object(source(destination_region, 1), 40, 9, Avatar, Root)).unwrap();
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

    let return_handoff = object(source(source_region, 104), 40, 10, Avatar, Root);
    assert_eq!(world.handoff_agent(destination_region, return_handoff).unwrap().outcome, AgentHandedOff(agent));
    let stale_property = PropertiesPatch { owner_id: Some(uuid(42)), creator_id: None };
    assert_eq!(patch_properties_current(&mut world, source(source_region, 101), agent_id, stale_property).unwrap().outcome, Unchanged);
    assert_eq!(world.object(agent).unwrap().properties().owner_id, Some(uuid(41)));

    assert!(matches!(world.retire_region(source_region).unwrap().outcome, world::RegionOutcome::Retired { entities_removed: 1, .. }));
    assert_eq!(world.entity(agent_id), None);
    assert!(world.region(source_region).is_none());
}
