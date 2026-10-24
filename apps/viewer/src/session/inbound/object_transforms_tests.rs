use super::*;

#[test]
fn decoded_xyz_rotations_pass_world_checks() {
    assert_observed_rotation_is_valid(vectors::OBJECT_UPDATE);
    assert_observed_rotation_is_valid(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
}

#[test]
fn xyz_decoder_tolerance_matches_world() {
    let inside = f32::from_bits(1.0_f32.to_bits() + 260);
    let outside = f32::from_bits(1.0_f32.to_bits() + 261);
    for fixture in [vectors::OBJECT_UPDATE, vectors::OBJECT_UPDATE_COMPRESSED_TYPED] {
        assert_xyz_rotation(fixture, inside, true);
        assert_xyz_rotation(fixture, outside, false);
    }
}

#[test]
fn terse_rotation_roundoff_is_accepted() {
    let patch = terse_patch(Quat::from_xyzw(-0.54654974, 0.48178887, 0.47275522, -0.4956464), 2);
    let decoded_rotation = patch.pose.expect("terse patch includes pose").rotation.expect("terse patch includes rotation");
    let norm_error = decoded_rotation.length_squared() - 1.0;
    assert!((0.00005..0.00006).contains(&norm_error), "unexpected quantized norm error: {norm_error}");
    let mut world = world();
    let base = ObjectObservation {
        pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
        ..ObjectObservation::new(
            UpdateSource::new(REGION, 1, OrderEvidence::Ordered(1), 0),
            Uuid::from_u128(0xdead),
            patch.local_id,
            Kind::Object,
            ParentUpdate::Root,
        )
    };
    let introduced = world.observe_object(base).expect("valid base object");
    let Introduced(entity) = introduced.outcome else { panic!("expected base object introduction") };

    let applied = world.patch_object(patch).expect("decoded terse rotation is valid");
    assert!(!applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation));
    let rotation = world.object(entity).expect("object remains present").rotation().expect("terse rotation is stored");
    assert!((rotation.length_squared() - 1.0).abs() <= 8.0 * f32::EPSILON);
}

#[test]
fn bad_terse_rotation_is_rejected() {
    let patch = terse_patch(Quat::from_xyzw(1.0, 1.0, 1.0, 1.0), 2);
    let decoded_rotation = patch.pose.expect("terse patch includes pose").rotation.expect("terse patch includes rotation");
    assert!(decoded_rotation.length_squared() > 3.9);
    let mut world = world();
    let base = ObjectObservation {
        pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
        ..ObjectObservation::new(
            UpdateSource::new(REGION, 1, OrderEvidence::Ordered(1), 0),
            Uuid::from_u128(0xdead),
            patch.local_id,
            Kind::Object,
            ParentUpdate::Root,
        )
    };
    let Introduced(entity) = world.observe_object(base).expect("valid base object").outcome else {
        panic!("expected base object introduction")
    };

    let applied = world.patch_object(patch).expect("invalid rotation is rejected as a field group");

    assert!(applied.rejections.iter().any(|rejection| rejection.reason == InvalidRotation));
    assert_eq!(world.object(entity).expect("base object remains").rotation(), Some(Quat::IDENTITY));
}

#[test]
fn compressed_update_keeps_parent_and_angular() {
    let packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let batch =
        decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(18), packet.message()).expect("compressed object batch");

    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.parent, ParentUpdate::Local(NonZeroU32::new(0x0506_0708).expect("nonzero parent ID")));
    assert_eq!(observation.angular_velocity, Some(Vec3::new(0.5, 0.0, -0.5)));
    assert_eq!(observation.linear_motion, None);
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(compressed_prim_shape())));
    assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
}

#[test]
fn prim_parameters_follow_ordered_world_updates() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full fixture") };
    update.objects.truncate(1);
    let id = update.objects[0].id;
    let local_id = update.objects[0].local_id;
    let mut world = world_with_regions(1);
    let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    let Introduced(entity) = world.observe_object(observation.clone()).unwrap().outcome else { panic!("expected introduction") };
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(full_prim_shape())));

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed fixture") };
    update.objects[0].id = id;
    update.objects[0].local_id = local_id;
    update.objects[0].shape = None;
    let batch = decode_object_message(REGION, 2, OrderEvidence::Ordered(2), &compressed).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    assert_eq!(observation.shape, FieldUpdate::Keep);
    let _ = world.observe_object(observation.clone()).unwrap();
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(full_prim_shape())));

    let ObjectUpdateCompressed(update) = &mut compressed else { unreachable!() };
    let ObjectUpdate(original) = &full else { unreachable!() };
    let mut shape = original.objects[0].shape;
    shape.path_curve = u8::MAX;
    shape.profile_curve = u8::MAX;
    shape.path_shear_x = 128;
    shape.path_shear_y = 127;
    shape.profile_hollow = u16::MAX;
    update.objects[0].shape = Some(shape);
    let expected = PrimShape {
        path_curve: u8::MAX,
        profile_curve: u8::MAX,
        path_shear_x: i8::MIN,
        path_shear_y: i8::MAX,
        profile_hollow: u16::MAX,
        ..full_prim_shape()
    };
    let batch = decode_object_message(REGION, 3, OrderEvidence::Ordered(3), &compressed).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(expected)));
    let _ = world.observe_object(observation.clone()).unwrap();
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(expected)));

    let revision = world.revision();
    let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).unwrap();
    let Some(ObjectOperation::Observe(observation)) = &batch.blocks[0] else { panic!("expected observation") };
    let _ = world.observe_object(observation.clone()).unwrap();
    assert_eq!(world.object(entity).unwrap().shape(), Some(Shape::Prim(expected)));
    assert_eq!(world.revision(), revision);
}
