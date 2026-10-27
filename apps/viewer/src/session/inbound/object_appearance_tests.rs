use super::*;
use world::TextureAnimation;

#[test]
fn texture_entries_apply_to_volumes_only() {
    let texture_entry = valid_texture_entry();

    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].texture_entry = texture_entry.clone();
    update.objects[1].texture_entry = texture_entry.clone();
    let full_batch = decode_object_message(REGION, 30, OrderEvidence::Ordered(30), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = full_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    assert_eq!(*object.surface_appearance.as_ref().unwrap().default_face(), FaceAppearance::default());
    let Some(ObjectOperation::Observe(avatar)) = full_batch.blocks[1].as_ref() else { panic!("expected avatar observation") };
    assert_eq!(avatar.kind, Kind::Avatar);
    assert_eq!(avatar.surface_appearance, None);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects[0].texture_entry = Some(texture_entry);
    let compressed_batch = decode_object_message(REGION, 31, OrderEvidence::Ordered(31), &compressed).expect("compressed object batch");
    let Some(ObjectOperation::Observe(object)) = compressed_batch.blocks[0].as_ref() else {
        panic!("expected compressed volume observation")
    };
    assert_eq!(*object.surface_appearance.as_ref().unwrap().default_face(), FaceAppearance::default());

    let mut terse = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected improved-terse fixture") };
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").texture_entry = packed_texture_entry();
    let terse_batch = decode_object_message(REGION, 32, OrderEvidence::Ordered(32), &terse).expect("terse object batch");
    let Some(ObjectOperation::Patch(patch)) = terse_batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert_eq!(*patch.surface_appearance.as_ref().unwrap().default_face(), FaceAppearance::default());

    let mut terse_avatar = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse_avatar else { panic!("expected improved-terse avatar fixture") };
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").texture_entry = packed_texture_entry();
    let avatar_batch = decode_object_message(REGION, 33, OrderEvidence::Ordered(33), &terse_avatar).expect("terse avatar batch");
    let Some(ObjectOperation::Patch(patch)) = avatar_batch.blocks[0].as_ref() else { panic!("expected terse avatar patch") };
    assert_eq!(patch.surface_appearance, None);
}

#[test]
fn texture_animation_updates_clear_and_preserve_by_path() {
    let mut wire = vec![0x01, 0xff, 2, 2];
    wire.extend_from_slice(&0.0_f32.to_le_bytes());
    wire.extend_from_slice(&0.0_f32.to_le_bytes());
    wire.extend_from_slice(&1.0_f32.to_le_bytes());
    let expected = TextureAnimation::from_wire(&wire).expect("valid texture animation block");

    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    if let ObjectUpdate(update) = &mut full {
        update.objects[0].texture_anim = wire.clone();
        update.objects[0].texture_entry = valid_texture_entry();
    } else {
        panic!("expected full object fixture");
    }
    let batch = decode_object_message(REGION, 35, OrderEvidence::Ordered(35), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };
    assert_eq!(object.texture_animation, FieldUpdate::Set(expected));

    if let ObjectUpdate(update) = &mut full {
        update.objects[0].texture_anim.clear();
    } else {
        panic!("expected full object fixture");
    }
    let batch = decode_object_message(REGION, 36, OrderEvidence::Ordered(36), &full).expect("full clear batch");
    let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };
    assert_eq!(object.texture_animation, FieldUpdate::Clear);

    if let ObjectUpdate(update) = &mut full {
        update.objects[0].texture_anim = wire[..15].to_vec();
    } else {
        panic!("expected full object fixture");
    }
    let batch = decode_object_message(REGION, 37, OrderEvidence::Ordered(37), &full).expect("malformed field batch");
    let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected full observation") };
    assert_eq!(batch.invalid_blocks, 0);
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(object.texture_animation, FieldUpdate::Keep);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    if let ObjectUpdateCompressed(update) = &mut compressed {
        update.objects[0].texture_animation = Some(wire.clone());
    } else {
        panic!("expected compressed object fixture");
    }
    let batch = decode_object_message(REGION, 38, OrderEvidence::Ordered(38), &compressed).expect("compressed object batch");
    let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(object.texture_animation, FieldUpdate::Set(expected));

    if let ObjectUpdateCompressed(update) = &mut compressed {
        update.objects[0].texture_animation = None;
    } else {
        panic!("expected compressed object fixture");
    }
    let batch = decode_object_message(REGION, 39, OrderEvidence::Ordered(39), &compressed).expect("compressed clear batch");
    let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected compressed observation") };
    assert_eq!(object.texture_animation, FieldUpdate::Clear);

    let terse = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let batch = decode_object_message(REGION, 40, OrderEvidence::Ordered(40), &terse).expect("terse object batch");
    let Some(ObjectOperation::Patch(patch)) = batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert_eq!(patch.texture_animation, FieldUpdate::Keep);

    let cached = packet(vectors::OBJECT_UPDATE_CACHED).message().clone();
    let batch = decode_object_message(REGION, 41, OrderEvidence::Ordered(41), &cached).expect("cached object batch");
    assert!(matches!(batch.blocks[0], Some(ObjectOperation::Cached { .. })), "cached references only request recovery");
}

#[test]
fn texture_flags_are_normalized() {
    for (flags, bump, shiny, fullbright, media_flags, mapping, media) in [
        (0x00, 0, 0, false, 0x00, TextureMapping::Default, false),
        (0x1f, 31, 0, false, 0x01, TextureMapping::Default, true),
        (0x20, 0, 0, true, 0x02, TextureMapping::Planar, false),
        (0x40, 0, 1, false, 0x03, TextureMapping::Planar, true),
        (0x80, 0, 2, false, 0x04, TextureMapping::Unsupported(2), false),
        (0xc0, 0, 3, false, 0x05, TextureMapping::Unsupported(2), true),
        (0xff, 31, 3, true, 0x06, TextureMapping::Unsupported(3), false),
        (0x25, 5, 0, true, 0xff, TextureMapping::Unsupported(3), true),
    ] {
        let mut texture_entry = valid_texture_entry();
        texture_entry[..16].copy_from_slice(Uuid::from_u128(1).as_bytes());
        texture_entry[17..21].copy_from_slice(&[127, 255, 0, 127]);
        texture_entry[22..26].copy_from_slice(&(-2.0_f32).to_le_bytes());
        texture_entry[27..31].copy_from_slice(&3.0_f32.to_le_bytes());
        texture_entry[41] = flags;
        texture_entry[43] = media_flags;
        texture_entry[47..63].copy_from_slice(Uuid::from_u128(2).as_bytes());
        let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.objects[0].texture_entry = texture_entry;
        let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).unwrap();
        let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
        assert_eq!(
            *object.surface_appearance.as_ref().unwrap().default_face(),
            FaceAppearance {
                texture: Some(asset_id(1)),
                color: [128, 0, 255, 128],
                transform: UvTransform { scale: [-2.0, 3.0], ..UvTransform::default() },
                bump,
                shiny,
                fullbright,
                mapping,
                media,
                material: Some(asset_id(2)),
                ..FaceAppearance::default()
            },
            "surface flags {flags:#x}, media flags {media_flags:#x}"
        );
    }
}

#[test]
fn clear_textures_are_absent() {
    for (texture, material, expected_material) in
        [(Uuid::nil(), Uuid::nil(), None), (Uuid::max(), Uuid::max(), Some(AssetId::new(Uuid::max())))]
    {
        let mut texture_entry = valid_texture_entry();
        texture_entry[..16].copy_from_slice(texture.as_bytes());
        texture_entry[47..63].copy_from_slice(material.as_bytes());
        let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.objects[0].texture_entry = texture_entry;
        let batch = decode_object_message(REGION, 1, OrderEvidence::Ordered(1), &full).unwrap();
        let Some(ObjectOperation::Observe(object)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
        let face = object.surface_appearance.as_ref().unwrap().default_face();
        assert_eq!(face.texture, None, "wire clear sentinel {texture}");
        assert_eq!(face.material, expected_material, "material assignments preserve non-nil IDs");
    }
}

#[test]
fn malformed_texture_entries_are_field_local() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].texture_entry = vec![0];
    update.objects[0].texture_anim.clear();

    let batch = decode_object_message(REGION, 34, OrderEvidence::Ordered(34), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };

    assert_eq!(batch.invalid_blocks, 0);
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(observation.surface_appearance, None);
    assert!(observation.pose.is_some());
    assert!(observation.scale.is_some());
    assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));
}

#[test]
fn embedded_materials_replace_and_clear() {
    let bindings = [(0, Uuid::from_u128(0x310)), (44, Uuid::from_u128(0x311))];

    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];
    let full_batch = decode_object_message(REGION, 40, OrderEvidence::Ordered(40), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = full_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let materials = object.pbr_materials.as_ref().expect("PBR parameter decoded");
    assert_eq!(materials.material(0), Some(asset_id(0x310)));
    assert_eq!(materials.material(44), Some(asset_id(0x311)));
    assert_eq!(materials.material(1), None);
    let mut state = world();
    let Introduced(entity) = state.observe_object(object.clone()).expect("PBR update seeds World").outcome else {
        panic!("expected volume object introduction")
    };
    let Some(ObjectOperation::Observe(avatar)) = full_batch.blocks[1].as_ref() else { panic!("expected avatar observation") };
    assert_eq!(avatar.pbr_materials, None);

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];
    let compressed_batch = decode_object_message(REGION, 41, OrderEvidence::Ordered(41), &compressed).expect("compressed batch");
    let Some(ObjectOperation::Observe(object)) = compressed_batch.blocks[0].as_ref() else {
        panic!("expected compressed volume observation")
    };
    assert_eq!(object.pbr_materials.as_ref().unwrap().material(44), Some(asset_id(0x311)));

    let mut absent = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut absent else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters.clear();
    let absent_batch = decode_object_message(REGION, 42, OrderEvidence::Ordered(42), &absent).expect("full object batch");
    let Some(ObjectOperation::Observe(object)) = absent_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    assert_eq!(object.pbr_materials, Some(PbrMaterialAssignments::default()));
    assert_eq!(state.observe_object(object.clone()).unwrap().outcome, Updated(entity));
    assert_eq!(state.object(entity).unwrap().pbr_materials(), None);

    let mut compressed_absent = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed_absent else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters.clear();
    let compressed_absent_batch =
        decode_object_message(REGION, 45, OrderEvidence::Ordered(45), &compressed_absent).expect("compressed batch");
    let Some(ObjectOperation::Observe(object)) = compressed_absent_batch.blocks[0].as_ref() else {
        panic!("expected compressed volume observation")
    };
    assert_eq!(object.pbr_materials, Some(PbrMaterialAssignments::default()));

    let mut terse = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
    let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected terse fixture") };
    update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").texture_entry = packed_texture_entry();
    let terse_batch = decode_object_message(REGION, 43, OrderEvidence::Ordered(43), &terse).expect("terse batch");
    let Some(ObjectOperation::Patch(patch)) = terse_batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert!(patch.surface_appearance.is_some());
    assert_eq!(patch.pbr_materials, None);
}

#[test]
fn non_volume_updates_clear_pbr_materials() {
    let bindings = [(0, Uuid::from_u128(0x320))];
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];

    let mut state = world();
    let initial_batch = decode_object_message(REGION, 50, OrderEvidence::Ordered(50), &full).expect("initial volume batch");
    let Some(ObjectOperation::Observe(initial)) = initial_batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let Introduced(entity) = state.observe_object(initial.clone()).expect("initial PBR object applies").outcome else {
        panic!("expected object introduction")
    };

    let mut full_non_volume = full.clone();
    let ObjectUpdate(update) = &mut full_non_volume else { panic!("expected full object fixture") };
    update.objects[0].pcode = 14;
    update.objects[0].extra_parameters.clear();
    let full_batch = decode_object_message(REGION, 51, OrderEvidence::Ordered(51), &full_non_volume).expect("full batch");
    let Some(ObjectOperation::Observe(observation)) = full_batch.blocks[0].as_ref() else { panic!("expected non-volume observation") };
    assert_eq!(observation.pbr_materials, Some(PbrMaterialAssignments::default()));
    assert_eq!(state.observe_object(observation.clone()).expect("full update applies").outcome, Updated(entity));
    assert_eq!(state.object(entity).expect("object remains").pbr_materials(), None);

    let restored_batch = decode_object_message(REGION, 52, OrderEvidence::Ordered(52), &full).expect("restored volume batch");
    let Some(ObjectOperation::Observe(observation)) = restored_batch.blocks[0].as_ref() else {
        panic!("expected restored volume observation")
    };
    assert_eq!(state.observe_object(observation.clone()).expect("restored PBR applies").outcome, Updated(entity));
    assert!(state.object(entity).expect("object remains").pbr_materials().is_some());

    let mut compressed_non_volume = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed_non_volume else { panic!("expected compressed object fixture") };
    update.objects[0].id = initial.id;
    update.objects[0].local_id = initial.local_id.get();
    update.objects[0].pcode = 14;
    update.objects[0].extra_parameters.clear();
    let compressed_batch = decode_object_message(REGION, 53, OrderEvidence::Ordered(53), &compressed_non_volume).expect("compressed batch");
    let Some(ObjectOperation::Observe(observation)) = compressed_batch.blocks[0].as_ref() else {
        panic!("expected compressed non-volume observation")
    };
    assert_eq!(observation.pbr_materials, Some(PbrMaterialAssignments::default()));
    assert_eq!(state.observe_object(observation.clone()).expect("compressed update applies").outcome, Updated(entity));
    assert_eq!(state.object(entity).expect("object remains").pbr_materials(), None);
}

#[test]
fn malformed_pbr_materials_are_field_local() {
    for parameters in
        [vec![ExtraParameter { parameter_type: 0x0080, data: vec![1] }], vec![ExtraParameter { parameter_type: 0x0080, data: vec![0, 0] }]]
    {
        let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.objects[0].extra_parameters = parameters;

        let batch = decode_object_message(REGION, 44, OrderEvidence::Ordered(44), &full).expect("full object batch");
        let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
        assert_eq!(observation.pbr_materials, None);
        assert!(observation.pose.is_some());
        assert!(observation.scale.is_some());
        assert_eq!(observation.shape, FieldUpdate::Set(Shape::Prim(full_prim_shape())));
    }
}

#[test]
fn u8_material_count_skips_extra_faces() {
    let bindings: Vec<_> = (0..usize::from(u8::MAX))
        .map(|index| {
            let face = if index < world::MAX_SURFACE_FACES { index as u8 } else { u8::MAX };
            (face, Uuid::from_u128(0x400 + index as u128))
        })
        .collect();

    let mut compressed = packet(vectors::OBJECT_UPDATE_COMPRESSED_TYPED).message().clone();
    let ObjectUpdateCompressed(update) = &mut compressed else { panic!("expected compressed object fixture") };
    update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];

    let encoded = Packet::new(45, compressed).with_zero_coding(false).encode().expect("full U8 count fits compressed ObjectUpdate");
    let packet = Packet::decode(&encoded).expect("decode full U8 count compressed ObjectUpdate");
    let batch = decode_object_message(REGION, 45, OrderEvidence::Ordered(45), packet.message()).expect("compressed object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let materials = observation.pbr_materials.as_ref().expect("PBR parameter decoded");
    assert_eq!(materials.material(44), Some(asset_id(0x42c)));
    assert_eq!(materials.material(45), None);
}

#[test]
fn full_materials_follow_variable1_limit() {
    for count in [14_usize, 15] {
        let bindings = (0..count).map(|face| (face as u8, Uuid::from_u128(0x500 + face as u128))).collect::<Vec<_>>();
        let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.objects[0].extra_parameters = vec![render_material_parameter(&bindings)];

        let encoded = Packet::new(46, full).with_zero_coding(false).encode();
        if count == 14 {
            let encoded = encoded.expect("14 records fit the full-update ExtraParams Variable 1 envelope");
            let packet = Packet::decode(&encoded).expect("decode 14-record full ObjectUpdate");
            let batch = decode_object_message(REGION, 46, OrderEvidence::Ordered(46), packet.message()).expect("full object batch");
            let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected full volume observation") };
            assert_eq!(observation.pbr_materials.as_ref().unwrap().bindings().count(), 14);
        } else {
            assert!(matches!(encoded, Err(protocol::LLUdpError::VariableFieldTooLong)));
        }
    }
}

#[test]
fn incremental_materials_replace_and_clear() {
    let mut state = world_with_regions(2);
    let object_id = Uuid::from_u128(0x600);
    let neighbor_id = Uuid::from_u128(0x601);
    let first = media_observation(REGION, object_id, 0x610);
    let Introduced(first_entity) = state.observe_object(first).expect("introduce addressed object").outcome else {
        panic!("expected object introduction")
    };
    let neighbor_region = Region::new(12);
    let _ = state.adopt_region(neighbor_region).expect("adopt neighbor region");
    let neighbor = media_observation(neighbor_region, neighbor_id, 0x620);
    let Introduced(neighbor_entity) = state.observe_object(neighbor).expect("introduce same local ID in neighbor").outcome else {
        panic!("expected neighbor object introduction")
    };

    let replacement = render_material_parameter(&[(44, Uuid::from_u128(0x630))]).data;
    let set = extra_param_message(0x0102_0304, 0x0080, true, replacement);
    let batch = apply_extra_param_message(&mut state, REGION, &set, 2);
    assert_eq!(batch.invalid_blocks, 0);
    {
        let object = state.object(first_entity).expect("addressed object remains");
        assert_eq!(object.pbr_materials().unwrap().material(0), None);
        assert_eq!(object.pbr_materials().unwrap().material(44), Some(asset_id(0x630)));
        assert_eq!(object.shape(), Some(Shape::Prim(Default::default())));
        assert_eq!(object.appearance(), Some(Appearance::Surface));
    }
    assert_eq!(state.object(neighbor_entity).unwrap().pbr_materials().unwrap().material(0), Some(asset_id(0x620)));

    let clear = extra_param_message(0x0102_0304, 0x0080, false, vec![1]);
    let batch = apply_extra_param_message(&mut state, REGION, &clear, 3);
    assert_eq!(batch.invalid_blocks, 0);
    {
        let object = state.object(first_entity).expect("addressed object remains");
        assert_eq!(object.pbr_materials(), None);
        assert_eq!(object.shape(), Some(Shape::Prim(Default::default())));
        assert_eq!(object.appearance(), Some(Appearance::Surface));
    }
    assert_eq!(state.object(neighbor_entity).unwrap().pbr_materials().unwrap().material(0), Some(asset_id(0x620)));

    let set_again = extra_param_message(0x0102_0304, 0x0080, true, render_material_parameter(&[(1, Uuid::from_u128(0x631))]).data);
    apply_extra_param_message(&mut state, REGION, &set_again, 4);
    let malformed = extra_param_message(0x0102_0304, 0x0080, true, vec![1]);
    let batch = apply_extra_param_message(&mut state, REGION, &malformed, 5);
    assert_eq!(batch.invalid_blocks, 1);
    assert!(batch.blocks[0].is_none());
    assert_eq!(state.object(first_entity).unwrap().pbr_materials().unwrap().material(1), Some(asset_id(0x631)));

    let unknown_clear = extra_param_message(0x0102_0304, 0x0030, false, Vec::new());
    apply_extra_param_message(&mut state, REGION, &unknown_clear, 6);
    assert_eq!(state.object(first_entity).unwrap().pbr_materials().unwrap().material(1), Some(asset_id(0x631)));
}

#[test]
fn repeated_material_params_replace_in_order() {
    let mut full = packet(vectors::OBJECT_UPDATE).message().clone();
    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].texture_entry = valid_texture_entry();
    update.objects[0].texture_anim.clear();
    update.objects[0].extra_parameters = vec![
        render_material_parameter(&[(0, Uuid::from_u128(0x313))]),
        ExtraParameter { parameter_type: 0x0080, data: vec![1] },
        render_material_parameter(&[(44, Uuid::from_u128(0x314))]),
    ];

    let batch = decode_object_message(REGION, 46, OrderEvidence::Ordered(46), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    let materials = observation.pbr_materials.as_ref().expect("PBR parameter decoded");
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(materials.material(0), None);
    assert_eq!(materials.material(44), Some(asset_id(0x314)));

    let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
    update.objects[0].extra_parameters =
        vec![render_material_parameter(&[(0, Uuid::from_u128(0x313))]), ExtraParameter { parameter_type: 0x0080, data: vec![1] }];
    update.objects[0].texture_anim.clear();
    let batch = decode_object_message(REGION, 47, OrderEvidence::Ordered(47), &full).expect("full object batch");
    let Some(ObjectOperation::Observe(observation)) = batch.blocks[0].as_ref() else { panic!("expected volume observation") };
    assert_eq!(batch.invalid_fields, 1);
    assert_eq!(observation.pbr_materials, None);
}

#[test]
fn terse_preserves_shape_and_appearance() {
    let packet = packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let batch = decode_object_message(REGION, packet.sequence(), OrderEvidence::Ordered(21), packet.message()).expect("terse batch");
    let Some(ObjectOperation::Patch(patch)) = batch.blocks[0].as_ref() else { panic!("expected terse patch") };
    assert_eq!(patch.shape, FieldUpdate::Keep);
    assert_eq!(patch.appearance, FieldUpdate::Keep);
}
