use super::*;

#[test]
fn full_update_fixture() {
    let (bytes, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message() else {
        panic!("expected full object update");
    };

    assert_eq!(packet.sequence(), 0x0102_0304);
    assert!(packet.is_zerocoded().expect("read zero-code flag"));
    assert_eq!(update.region_data, RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(update.objects.len(), 2);
    assert_eq!(
        update.objects[0],
        ObjectData {
            local_id: 0x1020_3040,
            state: 7,
            id: uuid("12345678-90ab-cdef-0123-456789abcdef"),
            crc: 0x7856_3412,
            pcode: 9,
            material: 4,
            click_action: 2,
            scale: Vec3::new(1.5, 2.0, 3.0),
            motion: ObjectMotion {
                position: Vec3::new(10.0, 20.0, 30.0),
                velocity: Vec3::new(1.0, 2.0, 3.0),
                acceleration: Vec3::new(-1.0, -2.0, -3.0),
                rotation: Quat::IDENTITY,
                angular_velocity: Vec3::new(0.5, 0.25, 0.0),
            },
            collision_plane: None,
            parent_local_id: 0x5566_7788,
            update_flags: 0x1234,
            shape: ObjectShape {
                path_curve: 16,
                profile_curve: 1,
                path_begin: 0x1000,
                path_end: 0xf000,
                path_scale_x: 128,
                path_scale_y: 129,
                path_shear_x: 1,
                path_shear_y: 255,
                path_twist: 2,
                path_twist_begin: -2,
                path_radius_offset: 3,
                path_taper_x: -3,
                path_taper_y: 4,
                path_revolutions: 252,
                path_skew: 18,
                profile_begin: 0x10fb,
                profile_end: 0x3020,
                profile_hollow: 0x5040,
            },
            texture_entry: vec![0xaa, 0x41, 0xbb],
            texture_anim: vec![0x10, 0x41],
            name_value: b"key\0value\0".to_vec(),
            data: vec![0xde, 0x41, 0x00, 0xbe],
            text: ObjectText { text: "hi".to_owned(), color: [1, 2, 3, 4] },
            media_url: "https://example".to_owned(),
            particle_system: vec![5, 0x41],
            extra_parameters: Vec::new(),
            sound: ObjectSound { sound_id: uuid("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"), gain: 0.5, flags: 1, radius: 20.0 },
            owner_id: uuid("ffeeddcc-bbaa-9988-7766-554433221100"),
            joint_type: 3,
            joint_pivot: Vec3::new(1.0, 2.0, 3.0),
            joint_axis_or_anchor: Vec3::new(-1.0, -2.0, -3.0),
        }
    );
    assert_eq!(
        update.objects[1],
        ObjectData {
            local_id: 0x0102_0304,
            state: 3,
            id: uuid("fedcba98-7654-3210-fedc-ba9876543210"),
            crc: 0xa1b2_c3d4,
            pcode: 47,
            material: 1,
            click_action: 0,
            scale: Vec3::new(0.5, 0.5, 1.0),
            motion: ObjectMotion {
                position: Vec3::new(10.0, 20.0, 30.0),
                velocity: Vec3::new(1.0, 2.0, 3.0),
                acceleration: Vec3::new(-1.0, -2.0, -3.0),
                rotation: Quat::IDENTITY,
                angular_velocity: Vec3::new(0.5, 0.25, 0.0),
            },
            collision_plane: Some(Vec4::new(0.0, 0.0, 1.0, 0.0)),
            parent_local_id: 0,
            update_flags: 0x8000_0000,
            shape: ObjectShape {
                path_curve: 16,
                profile_curve: 1,
                path_begin: 0x1000,
                path_end: 0xf000,
                path_scale_x: 128,
                path_scale_y: 129,
                path_shear_x: 1,
                path_shear_y: 255,
                path_twist: 2,
                path_twist_begin: -2,
                path_radius_offset: 3,
                path_taper_x: -3,
                path_taper_y: 4,
                path_revolutions: 252,
                path_skew: 18,
                profile_begin: 0x10fb,
                profile_end: 0x3020,
                profile_hollow: 0x5040,
            },
            texture_entry: vec![0xaa, 0x52, 0xbb],
            texture_anim: vec![0x10, 0x52],
            name_value: b"key\0value\0".to_vec(),
            data: vec![0xde, 0x52, 0x00, 0xbe],
            text: ObjectText { text: "hi".to_owned(), color: [1, 2, 3, 4] },
            media_url: "https://example".to_owned(),
            particle_system: vec![5, 0x52],
            extra_parameters: Vec::new(),
            sound: ObjectSound { sound_id: uuid("11111111-2222-3333-4444-555555555555"), gain: 0.5, flags: 1, radius: 20.0 },
            owner_id: uuid("ffeeddcc-bbaa-9988-7766-554433221100"),
            joint_type: 3,
            joint_pivot: Vec3::new(1.0, 2.0, 3.0),
            joint_axis_or_anchor: Vec3::new(-1.0, -2.0, -3.0),
        }
    );
    assert_reencodes(&packet, &bytes);
}

#[test]
fn full_update_typed_fields_round_trip() {
    let object = ObjectData {
        local_id: 0x0102_0304,
        state: 0x05,
        id: uuid("12345678-90ab-cdef-0123-456789abcdef"),
        crc: 0x1122_3344,
        pcode: 9,
        material: 3,
        click_action: 2,
        scale: Vec3::new(1.0, 2.0, 3.0),
        motion: ObjectMotion {
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::new(0.5, -1.5, 2.5),
            acceleration: Vec3::new(-0.25, 0.75, 1.25),
            rotation: Quat::from_xyzw(0.1, 0.2, 0.3, 0.927_361_85),
            angular_velocity: Vec3::new(3.0, 2.0, 1.0),
        },
        collision_plane: Some(Vec4::new(1.0, 2.0, 3.0, 4.0)),
        parent_local_id: 0x0506_0708,
        update_flags: 0xaabb_ccdd,
        shape: ObjectShape {
            path_curve: 1,
            profile_curve: 2,
            path_begin: 0x0304,
            path_end: 0x0506,
            path_scale_x: 7,
            path_scale_y: 8,
            path_shear_x: 9,
            path_shear_y: 10,
            path_twist: -11,
            path_twist_begin: -12,
            path_radius_offset: -13,
            path_taper_x: -14,
            path_taper_y: -15,
            path_revolutions: 16,
            path_skew: -17,
            profile_begin: 0x1213,
            profile_end: 0x1415,
            profile_hollow: 0x1617,
        },
        texture_entry: vec![0x10, 0x20, 0x30],
        texture_anim: vec![0x40, 0x50],
        name_value: vec![b'n', b'a', b'm', b'e', 0],
        data: vec![0x60, 0x70, 0x80],
        text: ObjectText { text: "hover".to_owned(), color: [0x11, 0x22, 0x33, 0x44] },
        media_url: "https://example.invalid".to_owned(),
        particle_system: vec![0x90, 0xa0],
        extra_parameters: vec![
            ExtraParameter { parameter_type: 0x1234, data: vec![0xb0, 0xc0, 0xd0] },
            ExtraParameter { parameter_type: 0x5678, data: vec![0xe0] },
        ],
        sound: ObjectSound { sound_id: uuid("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"), gain: 0.75, flags: 0x03, radius: 16.5 },
        owner_id: uuid("11111111-2222-3333-4444-555555555555"),
        joint_type: 4,
        joint_pivot: Vec3::new(41.5, 42.5, 43.5),
        joint_axis_or_anchor: Vec3::new(51.5, 52.5, 53.5),
    };
    let message = Message::ObjectUpdate(ObjectUpdate {
        region_data: RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 },
        objects: vec![object],
    });
    let sequence = 0x9988_7766;
    let encoded = Packet::new(sequence, message.clone()).with_zero_coding(false).encode().expect("encode typed full update");
    let decoded = Packet::decode(&encoded).expect("decode typed full update");

    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode typed full update"), encoded);

    let source = match &message {
        Message::ObjectUpdate(update) => &update.objects[0],
        _ => unreachable!(),
    };
    let id_offset = unique_offset(&encoded, &source.local_id.to_le_bytes());
    let mut fields_offset = id_offset + 40;
    fields_offset += 1 + usize::from(encoded[fields_offset]);
    fields_offset += 8 + 23;
    for (prefix, length) in [
        (2, source.texture_entry.len()),
        (1, source.texture_anim.len()),
        (2, source.name_value.len()),
        (2, source.data.len()),
        (1, source.text.text.len() + 1),
    ] {
        fields_offset += prefix + length;
    }
    fields_offset += 4;
    let extra_parameters_len = 1 + source.extra_parameters.iter().map(|parameter| 2 + 4 + parameter.data.len()).sum::<usize>();
    for (prefix, length) in [(1, source.media_url.len() + 1), (1, source.particle_system.len()), (1, extra_parameters_len)] {
        fields_offset += prefix + length;
    }
    fields_offset += 32;
    let gain_offset = fields_offset;
    let radius_offset = gain_offset + 5;
    let joint_pivot_offset = radius_offset + 5;
    let joint_axis_offset = joint_pivot_offset + 12;
    let collision_plane_offset = id_offset + 41;
    for (field, offset) in
        [gain_offset, radius_offset, joint_pivot_offset, joint_axis_offset, collision_plane_offset].into_iter().enumerate()
    {
        let mut malformed = encoded.clone();
        write_nan(&mut malformed, offset);
        let decoded = Packet::decode(&malformed).expect("decode non-finite semantic object values");
        let Message::ObjectUpdate(decoded) = decoded.message() else { panic!("expected full object update") };
        let object = &decoded.objects[0];
        let value_is_nan = match field {
            0 => object.sound.gain.is_nan(),
            1 => object.sound.radius.is_nan(),
            2 => object.joint_pivot.x.is_nan(),
            3 => object.joint_axis_or_anchor.x.is_nan(),
            _ => object.collision_plane.expect("collision plane remains present").x.is_nan(),
        };
        assert!(value_is_nan, "field {field} should retain its NaN value");
    }

    for field in 0..5 {
        let mut invalid = match message.clone() {
            Message::ObjectUpdate(update) => update,
            _ => unreachable!(),
        };
        match field {
            0 => invalid.objects[0].sound.gain = f32::NAN,
            1 => invalid.objects[0].sound.radius = f32::NAN,
            2 => invalid.objects[0].joint_pivot.x = f32::NAN,
            3 => invalid.objects[0].joint_axis_or_anchor.x = f32::NAN,
            _ => invalid.objects[0].collision_plane.as_mut().expect("fixture has a collision plane").x = f32::NAN,
        }
        assert_eq!(Packet::new(sequence, Message::ObjectUpdate(invalid)).with_zero_coding(false).encode(), Err(LLUdpError::InvalidMessage));
    }
}

#[test]
fn object_update_quaternions_use_canonical_xyz() {
    let rotation = Quat::from_xyzw(0.2, -0.3, 0.4, -0.84);

    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    update.objects[0].motion.rotation = rotation;
    let encoded = Packet::new(packet.sequence(), Message::ObjectUpdate(update.clone())).encode().expect("encode full rotation");
    let decoded = Packet::decode(&encoded).expect("decode full rotation");
    let Message::ObjectUpdate(decoded) = decoded.message() else {
        panic!("expected full object update");
    };
    let decoded_rotation = decoded.objects[0].motion.rotation;
    assert!(decoded_rotation.w >= 0.0);
    assert!((rotation.normalize().dot(decoded_rotation.normalize()).abs() - 1.0).abs() < 1.0e-5);

    update.objects[0].motion.rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 0.0);
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdate(update)).encode(), Err(LLUdpError::InvalidMessage));

    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(mut update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    update.objects[0].rotation = rotation;
    let encoded =
        Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update.clone())).encode().expect("encode compressed rotation");
    let decoded = Packet::decode(&encoded).expect("decode compressed rotation");
    let Message::ObjectUpdateCompressed(decoded) = decoded.message() else {
        panic!("expected compressed full object update");
    };
    let decoded_rotation = decoded.objects[0].rotation;
    assert!(decoded_rotation.w >= 0.0);
    assert!((rotation.normalize().dot(decoded_rotation.normalize()).abs() - 1.0).abs() < 1.0e-5);

    update.objects[0].rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 0.0);
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update)).encode(), Err(LLUdpError::InvalidMessage));
}

#[test]
fn full_update_extended_motion_is_accepted() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message() else {
        panic!("expected full object update");
    };
    let wire = Packet::new(packet.sequence(), Message::ObjectUpdate(update.clone()))
        .with_zero_coding(false)
        .encode()
        .expect("encode uncompressed full update fixture");
    let extension: Vec<u8> = (0..64).map(|byte| byte ^ 0xa5).collect();

    for object in &update.objects {
        let extended_wire = add_motion_extension(wire.clone(), object.local_id, &extension);
        let extended_packet = Packet::decode(&extended_wire).expect("decode extended full update");
        let Message::ObjectUpdate(extended) = extended_packet.message() else {
            panic!("expected full object update");
        };
        let extended_object =
            extended.objects.iter().find(|candidate| candidate.local_id == object.local_id).expect("object remains present");

        assert_eq!(extended_object.motion, object.motion);
        assert_eq!(extended_object.collision_plane, object.collision_plane);
        assert_eq!(extended_object, object);

        let canonical_wire = extended_packet.encode().expect("encode canonical motion layout");
        assert_ne!(canonical_wire, extended_wire);
        let canonical_packet = Packet::decode(&canonical_wire).expect("decode canonical motion layout");
        let Message::ObjectUpdate(canonical) = canonical_packet.message() else {
            panic!("expected full object update");
        };
        assert_eq!(canonical.objects.iter().find(|candidate| candidate.local_id == object.local_id), Some(object));
        assert_reencodes(&canonical_packet, &canonical_wire);
    }
}

#[test]
fn full_update_rejects_unsupported_motion_lengths() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message().clone() else {
        panic!("expected full object update");
    };

    let mut wire =
        Packet::new(packet.sequence(), Message::ObjectUpdate(update.clone())).with_zero_coding(false).encode().expect("encode full update");
    let first_id_offset = unique_offset(&wire, &update.objects[0].local_id.to_le_bytes());
    let motion_length_offset = first_id_offset + 40;

    for length in [59, 61, 75, 77, 123, 125, 139, 141] {
        wire[motion_length_offset] = length;
        assert_eq!(Packet::decode(&wire), Err(LLUdpError::InvalidMessage), "motion length {length}");
    }
}
