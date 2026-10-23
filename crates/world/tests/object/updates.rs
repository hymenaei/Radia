use super::support::{
    Ambiguous, AmbiguousOrder, AngularVelocity, Avatar, Deferred, Degraded, FieldGroup, FieldRejection, FieldRejectionReason, IgnoredStale,
    Introduced, InvalidRotation, KindMismatch, LinearMotion, Object, ObjectObservation, ObjectPatch, Pose, PropertiesPatch, Quat, Region,
    Removed, Root, Scale, Unchanged, Unknown, UnknownObject, UntrustedSource, UpdateSource, Updated, Uuid, Vec3, address, local_id, object,
    ordered, patch_properties_current, source, untracked, uuid, with_region,
};

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
        linear_motion: LinearMotion { velocity: Some(Vec3::splat(f32::INFINITY)), acceleration: None },
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
    assert_eq!(current.id(), id);
    assert_eq!(current.position(), Some(Vec3::X));
    assert_eq!(current.rotation(), Some(Quat::IDENTITY));
    assert_eq!(current.linear_motion(), LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    assert_eq!(current.angular_velocity(), Some(Vec3::Z));
    assert_eq!(current.scale(), Some(Vec3::ONE));
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
        patch_properties_current(
            &mut world,
            source(region, 11),
            uuid(1),
            PropertiesPatch { owner_id: Some(Uuid::nil()), creator_id: None }
        )
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
        assert_eq!(current.properties().owner_id, Some(Uuid::nil()));
    }

    let rotation = Quat::from_rotation_y(0.5);
    let patch = ObjectPatch {
        pose: Some(Pose { position: None, rotation: Some(rotation) }),
        linear_motion: LinearMotion { velocity: Some(Vec3::new(4.0, 0.0, 0.0)), acceleration: None },
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
        linear_motion: LinearMotion { velocity: Some(Vec3::splat(9.0)), acceleration: None },
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
fn untracked_updates_respect_field_authority() {
    let region = Region::new(32);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 320, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::X), rotation: None });
    initial.scale = Some(Vec3::ONE);
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected introduction") };
    let properties =
        patch_properties_current(&mut world, source(region, 2), uuid(320), PropertiesPatch { owner_id: Some(uuid(321)), creator_id: None })
            .unwrap();
    assert_eq!(properties.outcome, Updated(entity));

    let patch = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
        linear_motion: LinearMotion { velocity: Some(Vec3::Z), acceleration: None },
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
        linear_motion: LinearMotion { velocity: Some(Vec3::X), acceleration: None },
        ..ObjectPatch::new(source(region, 3), local_id(1))
    };
    assert_eq!(world.patch_object(older_tracked).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().linear_motion().velocity, Some(Vec3::X));

    let untrusted_properties = patch_properties_current(
        &mut world,
        untracked(region, 1_000),
        uuid(320),
        PropertiesPatch { owner_id: Some(uuid(322)), creator_id: Some(uuid(323)) },
    )
    .unwrap();
    assert_eq!(
        untrusted_properties.rejections.iter().copied().collect::<Vec<_>>(),
        [FieldRejection { group: FieldGroup::Properties, reason: UntrustedSource }]
    );
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(uuid(321)));
    assert_eq!(world.object(entity).unwrap().properties().creator_id, Some(uuid(323)));
    assert_eq!(world.remove_object(untracked(region, 1_001), local_id(1)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(320)), Some(entity));
    assert_eq!(world.observe_object(object(source(region, 4), 320, 2, Object, Root)).unwrap().outcome, Updated(entity));
    assert_eq!(world.entity_at(address(region, 1)), None);
    assert_eq!(world.entity_at(address(region, 2)), Some(entity));
    assert_eq!(world.remove_object(source(region, 5), local_id(2)).unwrap().outcome, Removed(uuid(320)));
    assert_eq!(world.object(entity).map(|object| object.id()), None);
}

#[test]
fn tracked_property_overrides_untracked() {
    let region = Region::new(45);
    let mut world = with_region(region);
    let object_id = uuid(450);
    let introduced = world.observe_object(object(source(region, 1), 450, 1, Object, Root)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let untracked_owner = uuid(451);
    let untracked = patch_properties_current(
        &mut world,
        untracked(region, 1_000),
        object_id,
        PropertiesPatch { owner_id: Some(untracked_owner), creator_id: None },
    )
    .unwrap();
    assert_eq!(untracked.outcome, Updated(entity));
    assert!(untracked.rejections.is_empty());
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(untracked_owner));
    let revision = world.revision();

    let tracked_owner = uuid(452);
    let tracked = patch_properties_current(
        &mut world,
        source(region, 2),
        object_id,
        PropertiesPatch { owner_id: Some(tracked_owner), creator_id: None },
    )
    .unwrap();

    assert_eq!(tracked.outcome, Updated(entity));
    assert!(tracked.rejections.is_empty());
    assert_eq!(tracked.revision.unwrap().get(), revision.get() + 1);
    assert_eq!(world.object(entity).unwrap().properties().owner_id, Some(tracked_owner));
}

#[test]
fn untracked_motion_cannot_replace_tracked() {
    let region = Region::new(31);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 310, 1, Object, Root);
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: None });
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let patch = ObjectPatch {
        linear_motion: LinearMotion { velocity: Some(Vec3::Y), acceleration: None },
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
    let patch = ObjectPatch {
        pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
        linear_motion: LinearMotion { velocity: Some(Vec3::Z), acceleration: None },
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
        patch_properties_current(&mut world, source(region, 115), uuid(70), PropertiesPatch { owner_id: Some(uuid(72)), creator_id: None })
            .unwrap()
            .outcome,
        Updated(entity)
    );
    let current = world.object(entity).unwrap();
    assert_eq!(current.transform().position(), Some(Vec3::Y));
    assert_eq!(current.transform().scale(), Some(Vec3::splat(2.0)));
    assert_eq!(current.properties().owner_id, Some(uuid(72)));
    let mut stale_alias = object(source(region, 115), 70, 11, Object, Root);
    stale_alias.pose = Some(Pose { position: Some(Vec3::splat(8.0)), rotation: None });
    stale_alias.scale = Some(Vec3::splat(8.0));
    assert_eq!(world.observe_object(stale_alias).unwrap().outcome, IgnoredStale);
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
    let applied = patch_properties_current(
        &mut world,
        source(region, 3),
        uuid(160),
        PropertiesPatch { owner_id: Some(uuid(161)), creator_id: Some(uuid(162)) },
    )
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
fn stale_property_targets_are_ignored() {
    let first = Region::new(56);
    let second = Region::new(57);
    let mut world = with_region(first);
    assert!(matches!(world.adopt_region(second).unwrap().outcome, world::RegionOutcome::Adopted { .. }));

    let object_id = uuid(560);
    let added = world.observe_object(object(source(first, 1), 560, 1, Object, Root)).unwrap();
    let Introduced(entity) = added.outcome else { panic!("expected object introduction") };
    let original_target = world.object_binding_target(entity).unwrap();

    assert_eq!(world.observe_object(object(source(second, 1), 560, 1, Object, Root)).unwrap().outcome, Updated(entity));
    assert_eq!(
        world
            .patch_properties(
                source(second, 2),
                original_target,
                object_id,
                PropertiesPatch { owner_id: Some(uuid(561)), creator_id: None },
            )
            .unwrap()
            .outcome,
        IgnoredStale
    );
    let current_target = world.object_binding_target(entity).unwrap();
    assert_eq!(
        world
            .patch_properties(
                source(second, 3),
                current_target,
                uuid(562),
                PropertiesPatch { owner_id: Some(uuid(563)), creator_id: None },
            )
            .unwrap()
            .outcome,
        IgnoredStale
    );
    assert!(world.entity(uuid(562)).is_none());

    let reused_target = world.object_binding_target(entity).unwrap();
    assert_eq!(world.remove_object(source(second, 4), local_id(1)).unwrap().outcome, Removed(object_id));
    let replacement = world.observe_object(object(source(second, 5), 560, 1, Object, Root)).unwrap();
    let Introduced(replacement_entity) = replacement.outcome else { panic!("expected replacement introduction") };
    assert_ne!(replacement_entity, entity);
    assert_eq!(
        world
            .patch_properties(source(second, 6), reused_target, object_id, PropertiesPatch { owner_id: Some(uuid(564)), creator_id: None },)
            .unwrap()
            .outcome,
        IgnoredStale
    );
    assert_eq!(world.object(replacement_entity).unwrap().properties().owner_id, None);
}

#[test]
fn ambiguous_properties_are_deferred() {
    let region = Region::new(50);
    let mut world = with_region(region);
    let patch = PropertiesPatch { owner_id: Some(uuid(501)), creator_id: None };
    assert!(world.entity(uuid(500)).and_then(|entity| world.object_binding_target(entity)).is_none());

    let entity = match world.observe_object(object(source(region, 2), 500, 1, Object, Root)).unwrap().outcome {
        Introduced(entity) => entity,
        other => panic!("expected object introduction, got {other:?}"),
    };
    let ambiguous = UpdateSource::new(region, 3, Ambiguous, 0);
    let applied = patch_properties_current(&mut world, ambiguous, uuid(500), patch).unwrap();

    assert_eq!(applied.outcome, Deferred(AmbiguousOrder));
    assert_eq!(applied.recovery.requests, [address(region, 1)]);
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

    let properties = patch_properties_current(
        &mut world,
        source(region, 200),
        uuid(120),
        PropertiesPatch { owner_id: Some(uuid(121)), creator_id: None },
    )
    .unwrap();
    assert_eq!(properties.outcome, Updated(entity));
    let patch = ObjectPatch { scale: Some(Vec3::splat(2.0)), ..ObjectPatch::new(source(region, 150), local_id(10)) };
    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().scale(), Some(Vec3::splat(2.0)));

    assert_eq!(world.remove_object(source(region, 190), local_id(10)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(120)), Some(entity));
}

#[test]
fn ambiguous_updates_defer_with_recovery() {
    let region = Region::new(2);
    let mut world = with_region(region);
    let expected_address = address(region, 7);
    let observation = object(source(region, 1), 2, 7, Object, Root);

    let ambiguous = UpdateSource::new(region, 1, Ambiguous, 0);
    let result = world.observe_object(ObjectObservation { source: ambiguous, ..observation.clone() }).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.requests, [expected_address]);

    let degraded = UpdateSource::new(region, 2, Degraded, 0);
    let result = world.observe_object(ObjectObservation { source: degraded, ..observation }).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.requests, [expected_address]);

    let result = world.patch_object(ObjectPatch::new(ambiguous, local_id(7))).unwrap();
    assert_eq!(result.outcome, Deferred(AmbiguousOrder));
    assert_eq!(result.recovery.requests, [expected_address]);
}

#[test]
fn empty_pose_patch_preserves_fence() {
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
