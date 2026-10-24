use super::support::{
    AmbiguousOrder, Avatar, Deferred, Degraded, FieldGroup, FieldRejection, FieldRejectionReason, HashSet, IgnoredStale, Introduced,
    InvalidValue, Local, Object, ObjectPatch, Parent, ParentCycle, ParentDependsOnRejectedParent, Pending, Pose, Quat, Region, Removed,
    Resolved, Root, SitTarget, Unchanged, UnknownObject, UntrustedSource, UpdateSource, Updated, Vec3, World, address, limits, local_id,
    object, source, untracked, uuid, with_region,
};

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
    assert!(unknown.recovery.requests.is_empty());

    let entity = match world.observe_object(object(source(region, 2), 490, 1, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let uncertain = UpdateSource::new(region, 3, Degraded, 0);
    let applied = world.patch_sit_target(uncertain, uuid(490), Some(target)).unwrap();

    assert_eq!(applied.outcome, Deferred(AmbiguousOrder));
    assert_eq!(applied.recovery.requests, [address(region, 1)]);
    assert_eq!(world.object(entity).unwrap().sit_target(), None);
}

#[test]
fn cyclic_parent_rejects_pose_but_preserves_siblings_and_blocks_older_declarations() {
    for use_observation in [false, true] {
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
        let applied = if use_observation {
            let mut observation = object(source(region, 50), 290, 1, Object, Local(local_id(2)));
            observation.pose = cyclic.pose;
            observation.scale = cyclic.scale;
            world.observe_object(observation).unwrap()
        } else {
            world.patch_object(cyclic).unwrap()
        };
        assert_eq!(applied.outcome, Updated(first_entity), "use_observation={use_observation}");
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
}

#[test]
fn transfer_pends_old_region_children() {
    let source_region = Region::new(33);
    let destination = Region::new(34);
    let mut world = World::new(limits());
    assert!(matches!(world.adopt_region(source_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    assert!(matches!(world.adopt_region(destination).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    let parent_address = address(source_region, 10);
    let child_address = address(source_region, 11);
    let parent = world.observe_object(object(source(source_region, 1), 330, 10, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let child = world.observe_object(object(source(source_region, 2), 331, 11, Object, Local(local_id(10)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };

    let moved_parent = world.observe_object(object(source(destination, 1), 330, 20, Object, Root)).unwrap();
    assert_eq!(moved_parent.outcome, Updated(parent_entity));
    assert_eq!(moved_parent.recovery.requests, [child_address]);
    assert_eq!(world.object(parent_entity).unwrap().address(), address(destination, 20));
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(uuid(330)) });

    let replacement = world.observe_object(object(source(source_region, 10), 332, 10, Object, Root)).unwrap();
    assert!(matches!(replacement.outcome, Introduced(_)));
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(uuid(330)) });
}

#[test]
fn rebind_preserves_an_unrelated_parent() {
    for parent_present in [false, true] {
        let region = Region::new(36);
        let mut world = with_region(region);
        let _ = world.observe_object(object(source(region, 1), 360, 1, Object, Root)).unwrap();
        if parent_present {
            let _ = world.observe_object(object(source(region, 2), 362, 3, Object, Root)).unwrap();
        }
        let child = world.observe_object(object(source(region, 3), 361, 2, Object, Local(local_id(3)))).unwrap();
        let Introduced(child) = child.outcome else { panic!("expected child introduction") };
        let parent = world.object(child).unwrap().parent();

        let rebound = world.observe_object(object(source(region, 4), 361, 1, Object, Local(local_id(3)))).unwrap();
        assert_eq!(rebound.outcome, Updated(child));
        assert_eq!(world.entity(uuid(360)), None);
        assert_eq!(world.object(child).unwrap().parent(), parent);
        if !parent_present {
            let _ = world.observe_object(object(source(region, 5), 362, 3, Object, Root)).unwrap();
        }
        let parent = world.entity(uuid(362)).unwrap();
        assert_eq!(world.object(child).unwrap().parent(), Resolved(parent));
        assert_eq!(world.children(parent).map(|object| object.entity()).collect::<Vec<_>>(), [child]);
    }
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
fn parent_cascade_respects_child_order() {
    let region = Region::new(3);
    let mut world = with_region(region);
    let parent_address = address(region, 1);
    let parent_id = uuid(30);
    let child_id = uuid(31);

    let child = world.observe_object(object(source(region, 10), 31, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: None });
    assert_eq!(child.recovery.requests, [parent_address]);

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
    assert_eq!(removed.recovery.requests, [address(region, 2)]);
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

    assert_eq!(world.observe_object(object(source(region, 14), 32, 2, Object, Root)).unwrap().outcome, IgnoredStale);
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
    assert_eq!(parent_applied.recovery.requests, [address(region, 1)]);
    assert_eq!(parent_applied.recovery_cancellations, [address(region, 3)]);

    let unrelated = world.observe_object(object(source(region, 4), 373, 2, Object, Root)).unwrap();
    let Introduced(unrelated) = unrelated.outcome else { panic!("expected local-ID replacement") };
    assert_eq!(world.entity(uuid(372)), None);
    assert_eq!(world.entity(uuid(373)), Some(unrelated));
    assert!(world.object(waiting).is_none());
    assert_eq!(world.object(first).unwrap().parent(), Pending { address: address(region, 2), expected_uuid: Some(uuid(372)) });
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
fn fresh_declaration_rebinds_pending_parent() {
    let region = Region::new(14);
    let mut world = with_region(region);
    let parent_address = address(region, 1);
    let parent_id = uuid(140);
    let parent = world.observe_object(object(source(region, 10), 140, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let child = world.observe_object(object(source(region, 11), 141, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };

    assert_eq!(
        world.patch_object(ObjectPatch { scale: Some(Vec3::ONE), ..ObjectPatch::new(source(region, 200), local_id(2)) }).unwrap().outcome,
        Updated(child_entity)
    );
    assert_eq!(world.remove_object(source(region, 12), local_id(1)).unwrap().outcome, Removed(parent_id));
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(parent_id) });

    assert_eq!(world.observe_object(object(source(region, 11), 141, 2, Object, Local(local_id(1)))).unwrap().outcome, IgnoredStale);
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
    std::assert_matches!(replacement.outcome, Introduced(_));
    assert_eq!(world.entity(uuid(143)), None);
    assert_eq!(world.object(child_entity).unwrap().parent(), Pending { address: parent_address, expected_uuid: Some(uuid(143)) });
}

#[test]
fn delayed_declaration_keeps_pending_parent() {
    let region = Region::new(17);
    let mut world = with_region(region);
    let parent_address = address(region, 1);
    let parent_id = uuid(170);

    let parent = world.observe_object(object(source(region, 10), 170, 1, Object, Root)).unwrap();
    std::assert_matches!(parent.outcome, Introduced(_));
    let child = world.observe_object(object(source(region, 11), 171, 2, Object, Local(local_id(1)))).unwrap();
    let Introduced(child_entity) = child.outcome else { panic!("expected child introduction") };
    assert_eq!(
        world.patch_object(ObjectPatch { scale: Some(Vec3::ONE), ..ObjectPatch::new(source(region, 200), local_id(2)) }).unwrap().outcome,
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
fn parent_address_reuse_needs_declaration() {
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
    assert_eq!(refreshed_parent.recovery.requests, [address(region, 2)]);
    assert_eq!(world.object(child).unwrap().parent(), Pending { address: parent_address, expected_uuid: None });

    assert_eq!(world.observe_object(object(source(region, 22), 192, 2, Object, Local(local_id(1)))).unwrap().outcome, Updated(child));
    assert_eq!(world.object(child).unwrap().parent(), Resolved(reused_parent));
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
fn parent_removal_keeps_seated_avatar_pending() {
    let region = Region::new(15);
    let mut world = with_region(region);
    let parent = world.observe_object(object(source(region, 10), 150, 1, Object, Root)).unwrap();
    let Introduced(parent_entity) = parent.outcome else { panic!("expected parent introduction") };
    let avatar = world.observe_object(object(source(region, 11), 151, 2, Avatar, Local(local_id(1)))).unwrap();
    let Introduced(avatar_entity) = avatar.outcome else { panic!("expected avatar introduction") };

    let removed = world.remove_object(source(region, 12), local_id(1)).unwrap();
    assert_eq!(removed.outcome, Removed(uuid(150)));
    assert!(world.object(parent_entity).is_none());
    assert_eq!(world.entity(uuid(151)), Some(avatar_entity));
    assert_eq!(world.object(avatar_entity).unwrap().parent(), Pending { address: address(region, 1), expected_uuid: Some(uuid(150)) });
    assert_eq!(removed.recovery.requests, [address(region, 2)]);
}

#[test]
fn cycle_checks_follow_parents_beyond_the_pose_limit() {
    for use_observation in [false, true] {
        let region = Region::new(200);
        let mut limits = limits();
        limits.max_entities = 34;
        limits.max_entities_per_region = 34;
        let mut world = World::new(limits);
        let _ = world.adopt_region(region).unwrap();
        let mut entities = Vec::new();
        for local in 1..=34 {
            let parent = if local == 1 { Root } else { Local(local_id(local - 1)) };
            let applied = world.observe_object(object(source(region, u64::from(local)), u128::from(local), local, Object, parent)).unwrap();
            let Introduced(entity) = applied.outcome else { panic!("expected chain introduction") };
            entities.push(entity);
        }
        let valid = ObjectPatch { parent: Some(Local(local_id(32))), ..ObjectPatch::new(source(region, 35), local_id(34)) };
        assert_eq!(world.patch_object(valid).unwrap().outcome, Updated(entities[33]));
        assert_eq!(world.object(entities[33]).unwrap().parent(), Resolved(entities[31]));
        assert_eq!(world.children(entities[32]).count(), 0);
        let revision = world.revision();
        let applied = if use_observation {
            world.observe_object(object(source(region, 36), 1, 1, Object, Local(local_id(34)))).unwrap()
        } else {
            world
                .patch_object(ObjectPatch { parent: Some(Local(local_id(34))), ..ObjectPatch::new(source(region, 36), local_id(1)) })
                .unwrap()
        };
        assert_eq!(applied.outcome, Unchanged);
        assert_eq!(
            applied.rejections.iter().copied().collect::<Vec<_>>(),
            [FieldRejection { group: FieldGroup::Parent, reason: ParentCycle }]
        );
        assert_eq!(world.revision(), revision);
        assert_eq!(world.object(entities[0]).unwrap().parent(), Parent::Root);
        for (index, entity) in entities.iter().enumerate().skip(1) {
            let parent = if index == 33 { entities[31] } else { entities[index - 1] };
            assert_eq!(world.object(*entity).unwrap().parent(), Resolved(parent));
        }
    }
}
