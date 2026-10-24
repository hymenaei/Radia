use super::*;

#[test]
fn compressed_shape_classification() {
    let fixture_packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    for (parameter_type, subtype, asset_uuid, expected) in [
        (
            0x0030,
            0x41,
            Uuid::from_u128(0x111),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x111),
                sculpt_type: SculptType::Sphere,
                flags: SculptFlags { invert: true, mirror: false },
            }),
        ),
        (
            0x0060,
            0x85,
            Uuid::from_u128(0x222),
            Some(Shape::Mesh { asset_id: asset_id(0x222), flags: SculptFlags { invert: false, mirror: true } }),
        ),
        (
            0x0030,
            0x42,
            Uuid::from_u128(0x224),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x224),
                sculpt_type: SculptType::Torus,
                flags: SculptFlags { invert: true, mirror: false },
            }),
        ),
        (
            0x0030,
            0x83,
            Uuid::from_u128(0x225),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x225),
                sculpt_type: SculptType::Plane,
                flags: SculptFlags { invert: false, mirror: true },
            }),
        ),
        (
            0x0030,
            0xc4,
            Uuid::from_u128(0x226),
            Some(Shape::Sculpt {
                asset_id: asset_id(0x226),
                sculpt_type: SculptType::Cylinder,
                flags: SculptFlags { invert: true, mirror: true },
            }),
        ),
        (0x0030, 0, Uuid::nil(), Some(Shape::Prim(compressed_prim_shape()))),
        (0x0030, 6, Uuid::from_u128(0x333), None),
        (
            0x0030,
            0xc7,
            Uuid::from_u128(0x333),
            Some(Shape::Sculpt {
                asset_id: asset_id(0xbe29_3869_d0d9_0a69_5989_ad27_f194_6fd4),
                sculpt_type: SculptType::Sphere,
                flags: SculptFlags::default(),
            }),
        ),
        (
            0x0030,
            0x09,
            Uuid::from_u128(0x333),
            Some(Shape::Sculpt {
                asset_id: asset_id(0xbe29_3869_d0d9_0a69_5989_ad27_f194_6fd4),
                sculpt_type: SculptType::Sphere,
                flags: SculptFlags::default(),
            }),
        ),
    ] {
        let mut data = vec![0; 17];
        data[..16].copy_from_slice(asset_uuid.as_bytes());
        data[16] = subtype;
        let mut message = fixture_packet.message().clone();
        let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
        update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type, data }];
        let batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(19), &message)
            .expect("compressed object batch");
        let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
        assert_eq!(
            observation.shape,
            expected.map_or(FieldUpdate::Clear, FieldUpdate::Set),
            "extra parameter type {parameter_type:#06x}, subtype {subtype}"
        );
        if subtype & 0x38 == 0
            && (1..=5).contains(&(subtype & 0x07))
            && let Some(Shape::Sculpt { asset_id, .. } | Shape::Mesh { asset_id, .. }) = expected
        {
            assert_eq!(asset_id.uuid(), asset_uuid);
            assert_ne!(observation.id, asset_uuid);
        }
        assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
    }

    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data: vec![0; 16] }];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Keep);

    let mut first_data = vec![0; 17];
    first_data[..16].copy_from_slice(Uuid::from_u128(0x444).as_bytes());
    first_data[16] = 1;
    let mut last_data = vec![0; 17];
    last_data[..16].copy_from_slice(Uuid::from_u128(0x555).as_bytes());
    last_data[16] = 5;
    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters =
        vec![ExtraParameter { parameter_type: 0x0030, data: first_data }, ExtraParameter { parameter_type: 0x0060, data: last_data }];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Mesh { asset_id: asset_id(0x555), flags: SculptFlags::default() }));

    let mut malformed_data = vec![0; 16];
    malformed_data[..16].copy_from_slice(Uuid::from_u128(0x666).as_bytes());
    let mut valid_data = vec![0; 17];
    valid_data[..16].copy_from_slice(Uuid::from_u128(0x777).as_bytes());
    valid_data[16] = 5;
    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![
        ExtraParameter { parameter_type: 0x0030, data: malformed_data.clone() },
        ExtraParameter { parameter_type: 0x0060, data: valid_data.clone() },
    ];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Mesh { asset_id: asset_id(0x777), flags: SculptFlags::default() }));

    let mut message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters =
        vec![ExtraParameter { parameter_type: 0x0060, data: valid_data }, ExtraParameter { parameter_type: 0x0030, data: malformed_data }];
    let batch =
        decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(20), &message).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(observation.shape, FieldUpdate::Keep);
}

#[test]
fn full_shape_classification() {
    let fixture_packet = packet(vectors::OBJECT_UPDATE);
    let mut regular = fixture_packet.message().clone();
    let Message::ObjectUpdate(update) = &mut regular else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    let batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(18), &regular).expect("full batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));

    let mut message = fixture_packet.message().clone();
    let Message::ObjectUpdate(update) = &mut message else { panic!("expected full object fixture") };
    update.objects.truncate(1);
    update.objects[0].pcode = 9;
    let mut data = vec![0; 17];
    data[..16].copy_from_slice(Uuid::from_u128(0x111).as_bytes());
    data[16] = 1;
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data }];

    let batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(19), &message).expect("full batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };

    assert_eq!(
        observation.shape,
        FieldUpdate::Set(Shape::Sculpt { asset_id: asset_id(0x111), sculpt_type: SculptType::Sphere, flags: SculptFlags::default() })
    );
    assert_eq!(observation.appearance, FieldUpdate::Set(Appearance::Surface));
}

#[test]
fn malformed_shape_params_preserve_shape() {
    let fixture_packet = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let mut valid_message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut valid_message else { panic!("expected compressed object fixture") };
    let mut sculpt_data = vec![0; 17];
    sculpt_data[..16].copy_from_slice(Uuid::from_u128(0x666).as_bytes());
    sculpt_data[16] = 0xc1;
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data: sculpt_data }];
    let valid_batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(22), &valid_message)
        .expect("compressed object batch");
    let Some(ObjectOperation::Observe(valid_observation)) = valid_batch.blocks[0].as_ref() else {
        panic!("expected compressed observation")
    };

    let mut world = world();
    let Introduced(entity) = world.observe_object(valid_observation.clone()).expect("valid sculpt update").outcome else {
        panic!("expected object introduction")
    };
    assert_eq!(
        world.object(entity).expect("object remains").shape(),
        Some(Shape::Sculpt {
            asset_id: asset_id(0x666),
            sculpt_type: SculptType::Sphere,
            flags: SculptFlags { invert: true, mirror: true },
        })
    );

    let mut malformed_message = fixture_packet.message().clone();
    let ObjectUpdateCompressed(update) = &mut malformed_message else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![ExtraParameter { parameter_type: 0x0030, data: vec![0; 16] }];
    update.objects[0].scale = Vec3::splat(2.0);
    let malformed_batch = decode_object_message(REGION, fixture_packet.sequence(), OrderEvidence::Ordered(23), &malformed_message)
        .expect("compressed object batch");
    let Some(ObjectOperation::Observe(malformed_observation)) = malformed_batch.blocks[0].as_ref() else {
        panic!("expected compressed observation")
    };
    assert_eq!(malformed_observation.shape, FieldUpdate::Keep);

    let applied = world.observe_object(malformed_observation.clone()).expect("unrelated valid fields still apply");
    assert!(matches!(applied.outcome, Updated(_)));
    assert_eq!(
        world.object(entity).expect("object remains").shape(),
        Some(Shape::Sculpt {
            asset_id: asset_id(0x666),
            sculpt_type: SculptType::Sphere,
            flags: SculptFlags { invert: true, mirror: true },
        })
    );
    assert_eq!(world.object(entity).expect("object remains").scale(), Some(Vec3::splat(2.0)));
}
