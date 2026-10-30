use super::support::{
    Ambiguous, AmbiguousOrder, AngularVelocity, Avatar, Deferred, Degraded, FieldGroup, FieldRejection, FieldRejectionReason, IgnoredStale,
    Introduced, InvalidRotation, KindMismatch, LinearMotion, MotionUpdateKind, Object, ObjectObservation, ObjectPatch, Pose, Quat, Region,
    Removed, Root, Scale, Unchanged, Unknown, UnknownObject, UntrustedSource, UpdateSource, Updated, Vec3, address, local_id, object,
    ordered, source, untracked, uuid, with_region,
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
        time_dilation: Some(f32::NAN),
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
            FieldRejection { group: FieldGroup::TimeDilation, reason: FieldRejectionReason::InvalidValue { field: "time_dilation" } },
            FieldRejection { group: Scale, reason: FieldRejectionReason::InvalidValue { field: "scale" } },
        ]
    );
    assert_eq!(applied.rejections.len(), 5);
    assert!(!applied.rejections.is_empty());
    let current = world.object(entity).unwrap();
    assert_eq!(current.id(), id);
    assert_eq!(current.position(), Some(Vec3::X));
    assert_eq!(current.rotation(), Some(Quat::IDENTITY));
    assert_eq!(current.linear_motion(), LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
    assert_eq!(current.angular_velocity(), Some(Vec3::Z));
    assert_eq!(current.time_dilation(), 1.0);
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
    {
        let current = world.object(entity).unwrap();
        assert_eq!(current.position(), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(current.rotation(), None);
        assert_eq!(current.scale(), Some(Vec3::splat(2.0)));
        assert_eq!(current.local_transform(), None);
        assert_eq!(current.linear_motion().velocity, Some(Vec3::X));
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
fn motion_metadata_is_stored_and_authoritative_updates_advance_the_sample() {
    let region = Region::new(72);
    let mut world = with_region(region);
    let first_source = source(region, 1);
    let mut initial = object(first_source, 720, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::ZERO) });
    initial.angular_velocity = Some(Vec3::Z);
    initial.is_physical = Some(true);
    initial.time_dilation = Some(0.5);
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    let current = world.object(entity).unwrap();
    assert!(current.is_physical());
    assert_eq!(current.time_dilation(), 0.5);
    let motion_source = current.motion_source().unwrap();
    assert_eq!((motion_source.region(), motion_source.sequence()), (first_source.region(), first_source.sequence()));
    assert!(motion_source.has_explicit_angular_velocity());

    let next_source = source(region, 2);
    let mut update = ObjectPatch::new(next_source, local_id(1));
    update.linear_motion = LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::ZERO) };
    update.angular_velocity = Some(Vec3::Z);
    update.is_physical = Some(true);
    update.time_dilation = Some(0.5);
    let applied = world.patch_object(update).unwrap();

    assert_eq!(applied.outcome, Updated(entity));
    let motion_source = world.object(entity).unwrap().motion_source().unwrap();
    assert_eq!((motion_source.region(), motion_source.sequence()), (next_source.region(), next_source.sequence()));
    assert!(motion_source.has_explicit_angular_velocity());
}

#[test]
fn motion_bounds_depend_on_update_kind() {
    let region = Region::new(73);
    let mut world = with_region(region);
    let mut initial = object(source(region, 1), 730, 1, Object, Root);
    initial.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
    initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::ZERO), acceleration: Some(Vec3::ZERO) });
    initial.angular_velocity = Some(Vec3::ZERO);
    let introduced = world.observe_object(initial).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let full_limit = ObjectPatch {
        linear_motion: LinearMotion { velocity: Some(Vec3::splat(4096.0)), acceleration: Some(Vec3::splat(-4096.0)) },
        angular_velocity: Some(Vec3::splat(4096.0)),
        ..ObjectPatch::new(source(region, 2), local_id(1))
    };
    assert_eq!(world.patch_object(full_limit).unwrap().outcome, Updated(entity));

    let full_out_of_range = ObjectPatch {
        linear_motion: LinearMotion { velocity: Some(Vec3::splat(f32::MAX)), acceleration: Some(Vec3::splat(-f32::MAX)) },
        angular_velocity: Some(Vec3::splat(f32::MAX)),
        ..ObjectPatch::new(source(region, 3), local_id(1))
    };
    let rejected = world.patch_object(full_out_of_range).unwrap();
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::LinearMotion, reason: FieldRejectionReason::InvalidValue { field: "velocity" } },
            FieldRejection { group: AngularVelocity, reason: FieldRejectionReason::InvalidValue { field: "angular_velocity" } },
        ]
    );
    assert_eq!(
        world.object(entity).unwrap().linear_motion(),
        LinearMotion { velocity: Some(Vec3::splat(4096.0)), acceleration: Some(Vec3::splat(-4096.0)) }
    );
    assert_eq!(world.object(entity).unwrap().angular_velocity(), Some(Vec3::splat(4096.0)));

    let terse_out_of_range = ObjectPatch {
        linear_motion: LinearMotion { velocity: Some(Vec3::splat(128.1)), acceleration: Some(Vec3::splat(-64.1)) },
        angular_velocity: Some(Vec3::splat(64.1)),
        ..ObjectPatch::new(source(region, 4).with_motion_update_kind(MotionUpdateKind::Terse), local_id(1))
    };
    let rejected = world.patch_object(terse_out_of_range).unwrap();
    assert_eq!(
        rejected.rejections.iter().copied().collect::<Vec<_>>(),
        [
            FieldRejection { group: FieldGroup::LinearMotion, reason: FieldRejectionReason::InvalidValue { field: "velocity" } },
            FieldRejection { group: AngularVelocity, reason: FieldRejectionReason::InvalidValue { field: "angular_velocity" } },
        ]
    );
    assert_eq!(
        world.object(entity).unwrap().linear_motion(),
        LinearMotion { velocity: Some(Vec3::splat(4096.0)), acceleration: Some(Vec3::splat(-4096.0)) }
    );
    assert_eq!(world.object(entity).unwrap().angular_velocity(), Some(Vec3::splat(4096.0)));
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
    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(applied.revision.unwrap().get(), revision.get() + 1);
    assert_eq!(world.revision().get(), revision.get() + 1);
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
    assert_eq!(applied.revision.unwrap().get(), revision.get() + 2);
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

    assert_eq!(applied.outcome, Updated(entity));
    assert_eq!(applied.revision.unwrap().get(), revision.get() + 1);
    assert_eq!(world.revision().get(), revision.get() + 1);
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

    assert_eq!(world.remove_object(untracked(region, 1_001), local_id(1)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(320)), Some(entity));
    assert_eq!(world.observe_object(object(source(region, 4), 320, 2, Object, Root)).unwrap().outcome, Updated(entity));
    assert_eq!(world.entity_at(address(region, 1)), None);
    assert_eq!(world.entity_at(address(region, 2)), Some(entity));
    assert_eq!(world.remove_object(source(region, 5), local_id(2)).unwrap().outcome, Removed(uuid(320)));
    assert_eq!(world.object(entity).map(|object| object.id()), None);
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

    let current = world.object(entity).unwrap();
    assert_eq!(current.transform().position(), Some(Vec3::Y));
    assert_eq!(current.transform().scale(), Some(Vec3::splat(2.0)));
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
    assert_eq!(same_values.outcome, Updated(entity));
    assert_eq!(same_values.revision.unwrap().get(), revision_before_same_values.get() + 1);
    assert_eq!(world.revision().get(), revision_before_same_values.get() + 1);
    assert_eq!(world.remove_object(source(region, 125), local_id(10)).unwrap().outcome, Unchanged);
    assert_eq!(world.entity(uuid(70)), Some(entity));
}

#[test]
fn observation_patch_advances_revision() {
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
    assert_eq!(world.revision().get(), revision + 1);
    assert_eq!(world.object(entity).unwrap().position(), Some(Vec3::X));
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

    let motion = LinearMotion { velocity: Some(Vec3::Y), acceleration: None };
    let patch = ObjectPatch { linear_motion: motion, ..ObjectPatch::new(source(region, 200), local_id(10)) };
    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));
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
