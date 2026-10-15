use base::UUID;
use math::{Quat, Vec3, Vec4};
use protocol::{
    LLUdpError,
    lludp::{Message, Packet},
    messages::{
        CacheMissType, CachedObjectReference, ExtraParameter, KillObject, ObjectData, ObjectMotion, ObjectRequest, ObjectShape,
        ObjectSound, ObjectSpecificData, ObjectText, ObjectUpdate, ObjectUpdateCached, RegionData, RequestAgentData,
        RequestMultipleObjects, kill_object,
    },
};
use testing::{lludp::vectors, parse_hex_fixture};

fn fixture(hex: &str) -> Vec<u8> {
    parse_hex_fixture(hex).expect("valid protocol fixture")
}

fn unique_offset(bytes: &[u8], needle: &[u8]) -> usize {
    let mut offsets = bytes.windows(needle.len()).enumerate().filter_map(|(offset, candidate)| (candidate == needle).then_some(offset));
    let offset = offsets.next().expect("fixture field is present");
    assert!(offsets.next().is_none(), "fixture field {needle:02x?} is unique");
    offset
}

fn write_nan(bytes: &mut [u8], offset: usize) {
    bytes[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
}

fn add_motion_extension(mut bytes: Vec<u8>, local_id: u32, extension: &[u8]) -> Vec<u8> {
    let local_id_offset = unique_offset(&bytes, &local_id.to_le_bytes());
    let motion_length_offset = local_id_offset + 40;
    let motion_length = usize::from(bytes[motion_length_offset]);
    let extended_length = motion_length + extension.len();
    bytes[motion_length_offset] = u8::try_from(extended_length).expect("extended motion block fits Variable 1");
    let motion_end = motion_length_offset + 1 + motion_length;
    bytes.splice(motion_end..motion_end, extension.iter().copied());
    bytes
}

fn write_midpoint_components(bytes: &mut [u8], offset: usize, count: usize) {
    let (components, _) = bytes[offset..offset + count * 2].as_chunks_mut::<2>();
    for (index, component) in components.iter_mut().enumerate() {
        let midpoint = 32_767 + (index % 2) as u16;
        component.copy_from_slice(&midpoint.to_le_bytes());
    }
}

fn uuid(value: &str) -> UUID {
    UUID::parse_str(value).expect("valid fixture UUID")
}

fn decode(hex: &str) -> (Vec<u8>, Packet) {
    let bytes = fixture(hex);
    let packet = Packet::decode(&bytes).expect("decode independent fixture");
    (bytes, packet)
}

fn assert_reencodes(packet: &Packet, bytes: &[u8]) {
    assert_eq!(packet.encode().expect("re-encode fixture"), bytes);
}

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

#[test]
fn compressed_update_fixture() {
    let (bytes, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(update) = packet.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects.len(), 1);
    let object = &update.objects[0];
    assert_eq!(update.region_data, RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(object.update_flags, 0x1234_5678);
    assert_eq!(object.local_id, 0x0102_0304);
    assert_eq!(object.id, uuid("12345678-90ab-cdef-0123-456789abcdef"));
    assert_eq!(object.pcode, 9);
    assert_eq!(object.state, 3);
    assert_eq!(object.crc, 0x1122_3344);
    assert_eq!(object.material, 2);
    assert_eq!(object.click_action, 1);
    assert_eq!(object.scale, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(object.position, Vec3::new(4.0, 5.0, 6.0));
    assert_eq!(object.rotation, Quat::IDENTITY);
    assert_eq!(object.angular_velocity, Some(Vec3::new(0.5, 0.0, -0.5)));
    assert_eq!(object.special_code, 0x07fe);
    assert_eq!(object.owner_id, uuid("00112233-4455-6677-8899-aabbccddeeff"));
    assert_eq!(object.parent_local_id, 0x0506_0708);
    assert_eq!(object.data, Some(ObjectSpecificData::Tree { species: 3 }));
    assert_eq!(object.text, Some(ObjectText { text: "hello".into(), color: [1, 2, 3, 4] }));
    assert_eq!(object.media_url.as_deref(), Some("https://example.invalid"));
    assert_eq!(object.legacy_particle_system.as_deref(), Some(&[0x5a; 86][..]));
    assert_eq!(
        object.extra_parameters,
        [
            ExtraParameter { parameter_type: 0x1234, data: vec![0xaa, 0xbb, 0xcc] },
            ExtraParameter { parameter_type: 0x5678, data: Vec::new() },
        ]
    );
    assert_eq!(
        object.sound,
        Some(ObjectSound { sound_id: uuid("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"), gain: 0.75, flags: 3, radius: 16.5 })
    );
    assert_eq!(object.name_value.as_deref(), Some("Name=Crate\nDesc=Typed"));
    assert_eq!(
        object.shape,
        Some(ObjectShape {
            path_curve: 1,
            profile_curve: 16,
            path_begin: 258,
            path_end: 772,
            path_scale_x: 5,
            path_scale_y: 6,
            path_shear_x: 7,
            path_shear_y: 8,
            path_twist: -9,
            path_twist_begin: -10,
            path_radius_offset: -11,
            path_taper_x: -12,
            path_taper_y: -13,
            path_revolutions: 14,
            path_skew: -15,
            profile_begin: 4370,
            profile_end: 4884,
            profile_hollow: 5398,
        })
    );
    assert_eq!(object.texture_entry.as_deref(), Some(&[0xde, 0xad, 0, 0xbe, 0xef][..]));
    assert_eq!(object.texture_animation.as_deref(), Some(&(0u8..16).collect::<Vec<_>>()[..]));
    let mut particle_system = vec![0x44, 0, 0, 0];
    particle_system.extend([0x11; 68]);
    particle_system.extend([0x12, 0, 0, 0]);
    particle_system.extend([0x22; 18]);
    assert_eq!(object.particle_system.as_deref(), Some(particle_system.as_slice()));
    assert_reencodes(&packet, &bytes);

    let mut malformed_sound = bytes;
    let sound_id = object.sound.expect("fixture sound").sound_id;
    let sound_offset = unique_offset(&malformed_sound, sound_id.as_bytes());
    write_nan(&mut malformed_sound, sound_offset + 16);
    write_nan(&mut malformed_sound, sound_offset + 21);
    let decoded = Packet::decode(&malformed_sound).expect("decode non-finite compressed sound values");
    let Message::ObjectUpdateCompressed(decoded) = decoded.message() else { panic!("expected compressed object update") };
    let sound = decoded.objects[0].sound.expect("sound remains present");
    assert!(sound.gain.is_nan());
    assert!(sound.radius.is_nan());
}

#[test]
fn compressed_update_ignores_unknown_object_suffix() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    let mut wire = Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update.clone()))
        .with_zero_coding(false)
        .encode()
        .expect("encode compressed object update");

    let local_id_offset = unique_offset(&wire, &update.objects[0].local_id.to_le_bytes());
    let object_data_start = local_id_offset - 16;
    let object_data_len_offset = object_data_start - 2;
    let object_data_len =
        usize::from(u16::from_le_bytes(wire[object_data_len_offset..object_data_start].try_into().expect("object data length")));
    let object_data_end = object_data_start + object_data_len;
    let extension = [0xde, 0xad, 0xbe, 0xef];
    wire.splice(object_data_end..object_data_end, extension);
    let new_object_data_len = u16::try_from(object_data_len + extension.len()).expect("object data length fits wire field");
    wire[object_data_len_offset..object_data_start].copy_from_slice(&new_object_data_len.to_le_bytes());

    let preflight = Packet::preflight(&wire).expect("preflight ignores unknown object suffix");
    let decoded = Packet::decode(&wire).expect("decode ignores unknown object suffix");
    assert!(preflight.retained_capacity_upper_bound() >= decoded.retained_capacity_bytes());
    let Message::ObjectUpdateCompressed(decoded) = decoded.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(decoded.objects.len(), 1);
    assert_eq!(decoded.objects[0].local_id, update.objects[0].local_id);
    assert_eq!(decoded.objects[0].shape, update.objects[0].shape);
}

#[test]
fn terse_update_fixtures_are_partial() {
    let (bytes, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.region_data, RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(update.objects.len(), 1);
    assert_eq!(update.objects[0].local_id, 0x1020_3040);
    assert_eq!(update.objects[0].state, 5);
    assert!(!update.objects[0].avatar);
    assert_eq!(update.objects[0].collision_plane, None);
    assert_eq!(update.objects[0].motion.position, Vec3::new(5.0, 6.0, 7.0));
    assert_eq!(update.objects[0].motion.velocity, Vec3::new(-128.0, 0.0, 128.0));
    assert_eq!(update.objects[0].motion.acceleration, Vec3::new(-64.0, 0.0, 64.0));
    assert_eq!(update.objects[0].motion.rotation, Quat::from_xyzw(-1.0, 0.0, 1.0, 0.0));
    assert_eq!(update.objects[0].motion.angular_velocity, Vec3::new(-64.0, 0.0, 64.0));
    assert_eq!(update.objects[0].texture_entry.as_slice(), &[0x11, 0, 0x22]);
    let canonical = packet.encode().expect("re-encode terse object update");
    assert_ne!(canonical, bytes, "zero snap canonicalizes the near-zero quantized value");
    assert_eq!(Packet::decode(&canonical).expect("decode canonical terse update").message(), packet.message());

    let (avatar_bytes, avatar_packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let Message::ImprovedTerseObjectUpdate(avatar_update) = avatar_packet.message() else {
        panic!("expected terse avatar update");
    };
    assert_eq!(avatar_update.region_data, RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(avatar_update.objects.len(), 1);
    let avatar = &avatar_update.objects[0];
    assert_eq!(avatar.local_id, 0x5566_7788);
    assert_eq!(avatar.state, 9);
    assert!(avatar.avatar);
    assert_eq!(avatar.collision_plane, Some(Vec4::new(0.0, 0.0, 1.0, 0.0)));
    assert_eq!(avatar.motion.position, Vec3::new(8.0, 9.0, 10.0));
    assert_eq!(avatar.motion.velocity, Vec3::new(-128.0, 0.0, 128.0));
    assert_eq!(avatar.motion.acceleration, Vec3::new(-64.0, 0.0, 64.0));
    assert_eq!(avatar.motion.rotation, Quat::IDENTITY);
    assert_eq!(avatar.motion.angular_velocity, Vec3::new(-64.0, 0.0, 64.0));
    assert!(avatar.texture_entry.is_empty());
    assert_reencodes(&avatar_packet, &avatar_bytes);
}

#[test]
fn terse_avatar_plane_and_empty_texture_entry_are_consistent() {
    let (_, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected terse object update");
    };
    update.objects[0].texture_entry.clear();
    let encoded =
        Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update.clone())).encode().expect("encode empty texture entry");
    let decoded = Packet::decode(&encoded).expect("decode empty texture entry");
    let Message::ImprovedTerseObjectUpdate(decoded) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert!(decoded.objects[0].texture_entry.is_empty());

    update.objects[0].avatar = true;
    assert_eq!(
        Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update.clone())).encode(),
        Err(LLUdpError::InvalidMessage),
    );

    update.objects[0].avatar = false;
    update.objects[0].collision_plane = Some(Vec4::ZERO);
    assert_eq!(Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update)).encode(), Err(LLUdpError::InvalidMessage));
}

#[test]
fn terse_quantized_midpoint_decodes_as_exact_zero() {
    let mut bytes = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let local_id = 0x1020_3040_u32.to_le_bytes();
    let local_id_offset = unique_offset(&bytes, &local_id);
    write_midpoint_components(&mut bytes, local_id_offset + 18, 13);

    let packet = Packet::decode(&bytes).expect("decode midpoint motion values");
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    let motion = &update.objects[0].motion;
    assert_eq!(motion.velocity, Vec3::ZERO);
    assert_eq!(motion.acceleration, Vec3::ZERO);
    assert_eq!(motion.rotation, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
    assert_eq!(motion.angular_velocity, Vec3::ZERO);

    let encoded = Packet::new(packet.sequence(), packet.message().clone()).encode().expect("re-encode zero motion");
    let decoded = Packet::decode(&encoded).expect("decode re-encoded zero motion");
    let Message::ImprovedTerseObjectUpdate(update) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.objects[0].motion.velocity, Vec3::ZERO);
    assert_eq!(update.objects[0].motion.acceleration, Vec3::ZERO);
    assert_eq!(update.objects[0].motion.rotation, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
    assert_eq!(update.objects[0].motion.angular_velocity, Vec3::ZERO);
}

#[test]
fn cached_and_kill_fixtures() {
    let (cache_bytes, cache_packet) = decode(vectors::OBJECT_UPDATE_CACHED);
    assert_eq!(
        cache_packet.message(),
        &Message::ObjectUpdateCached(ObjectUpdateCached {
            region_data: RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 },
            objects: vec![
                CachedObjectReference { local_id: 0x0102_0304, crc: 0x1122_3344, update_flags: 0x5566_7788 },
                CachedObjectReference { local_id: 0x1020_3040, crc: 0xaabb_ccdd, update_flags: 0x8765_4321 },
            ],
        })
    );
    assert_reencodes(&cache_packet, &cache_bytes);

    let (kill_bytes, kill_packet) = decode(vectors::KILL_OBJECT);
    assert_eq!(
        kill_packet.message(),
        &Message::KillObject(KillObject {
            object_data: vec![kill_object::ObjectData { local_id: 0x0102_0304 }, kill_object::ObjectData { local_id: 0x5566_7788 },],
        })
    );
    assert_reencodes(&kill_packet, &kill_bytes);
}

#[test]
fn request_multiple_objects_conversion() {
    let (bytes, packet) = decode(vectors::REQUEST_MULTIPLE_OBJECTS);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: uuid("ffeeddcc-bbaa-9988-7766-554433221100"),
            session_id: uuid("00112233-4455-6677-8899-aabbccddeeff"),
        },
        objects: vec![
            ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 0x0102_0304 },
            ObjectRequest { cache_miss_type: CacheMissType::CrcMismatch, local_id: 0x5566_7788 },
        ],
    };
    assert_eq!(packet.message(), &Message::RequestMultipleObjects(request.clone()));
    assert!(packet.is_reliable());
    assert_reencodes(&packet, &bytes);

    let converted: Message = request.clone().into();
    let encoded = Packet::new(7, converted).with_reliable().encode().expect("encode converted request");
    assert_eq!(Packet::decode(&encoded).expect("decode converted request").message(), &Message::RequestMultipleObjects(request));
}

#[test]
fn object_fixtures_reject_every_truncated_prefix() {
    for wire in [
        vectors::OBJECT_UPDATE,
        vectors::OBJECT_UPDATE_COMPRESSED_TYPED,
        vectors::IMPROVED_TERSE_OBJECT_UPDATE,
        vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR,
        vectors::OBJECT_UPDATE_CACHED,
        vectors::KILL_OBJECT,
        vectors::REQUEST_MULTIPLE_OBJECTS,
    ] {
        let packet = Packet::decode(&fixture(wire)).expect("decode object fixture");
        let message = packet.message().clone();
        let empty_message = empty_object_message(&message);
        let encode = |message| Packet::new(packet.sequence(), message).with_zero_coding(false).encode().expect("encode object message");
        let bytes = encode(message);
        let empty_bytes = encode(empty_message.clone());
        let omitted_count_offset = empty_bytes.len() - 1;

        let empty = Packet::decode(&bytes[..omitted_count_offset]).expect("an omitted terminal count means zero objects");
        assert_eq!(empty.message(), &empty_message);

        for end in 0..bytes.len() {
            if end == omitted_count_offset {
                continue;
            }
            assert!(Packet::decode(&bytes[..end]).is_err(), "accepted a {}-byte prefix of a {}-byte packet", end, bytes.len());
        }
    }
}

fn empty_object_message(message: &Message) -> Message {
    match message {
        Message::ObjectUpdate(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ObjectUpdate(update)
        }
        Message::ObjectUpdateCompressed(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ObjectUpdateCompressed(update)
        }
        Message::ImprovedTerseObjectUpdate(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ImprovedTerseObjectUpdate(update)
        }
        Message::ObjectUpdateCached(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::ObjectUpdateCached(update)
        }
        Message::KillObject(update) => {
            let mut update = update.clone();
            update.object_data.clear();
            Message::KillObject(update)
        }
        Message::RequestMultipleObjects(update) => {
            let mut update = update.clone();
            update.objects.clear();
            Message::RequestMultipleObjects(update)
        }
        other => panic!("not an object message: {other:?}"),
    }
}

#[test]
fn malformed_object_transforms_and_optional_data_fail() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    update.objects[0].scale.x = f32::NAN;
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdate(update)).encode(), Err(LLUdpError::InvalidMessage));

    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    let mut malformed =
        Packet::new(packet.sequence(), Message::ObjectUpdate(update)).with_zero_coding(false).encode().expect("encode full update");
    malformed[58] = 61;
    assert_eq!(Packet::decode(&malformed), Err(LLUdpError::InvalidMessage));

    let mut truncated = fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    truncated[22..24].copy_from_slice(&84u16.to_le_bytes());
    truncated.truncate(24 + 84);
    assert_eq!(Packet::decode(&truncated), Err(LLUdpError::TruncatedPacket));
}

#[test]
fn decoder_keeps_non_finite_values() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    let local_id = update.objects[0].local_id.to_le_bytes();
    let mut full = Packet::new(packet.sequence(), Message::ObjectUpdate(update))
        .with_zero_coding(false)
        .encode()
        .expect("encode uncompressed full update");
    let local_id_offset = unique_offset(&full, &local_id);
    write_nan(&mut full, local_id_offset + 28);
    let decoded = Packet::decode(&full).expect("decode full update with a rejected world field");
    let Message::ObjectUpdate(decoded) = decoded.message() else { panic!("expected full object update") };
    assert!(decoded.objects[0].scale.x.is_nan());

    let mut compressed = fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let local_id = 0x0102_0304_u32.to_le_bytes();
    let local_id_offset = unique_offset(&compressed, &local_id);
    write_nan(&mut compressed, local_id_offset + 24);
    let decoded = Packet::decode(&compressed).expect("decode compressed update with a rejected world field");
    let Message::ObjectUpdateCompressed(decoded) = decoded.message() else { panic!("expected compressed object update") };
    assert!(decoded.objects[0].position.x.is_nan());

    let mut terse = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let local_id = 0x1020_3040_u32.to_le_bytes();
    let local_id_offset = unique_offset(&terse, &local_id);
    write_nan(&mut terse, local_id_offset + 6);
    let decoded = Packet::decode(&terse).expect("decode terse update with a rejected world field");
    let Message::ImprovedTerseObjectUpdate(decoded) = decoded.message() else { panic!("expected terse object update") };
    assert!(decoded.objects[0].motion.position.x.is_nan());

    let (_, avatar_packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let Message::ImprovedTerseObjectUpdate(avatar_update) = avatar_packet.message() else {
        panic!("expected terse avatar update");
    };
    let local_id = avatar_update.objects[0].local_id.to_le_bytes();
    let mut avatar = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let local_id_offset =
        avatar.windows(local_id.len()).rposition(|candidate| candidate == local_id.as_slice()).expect("avatar local ID is present");
    write_nan(&mut avatar, local_id_offset + 6);
    let decoded = Packet::decode(&avatar).expect("decode avatar update with a non-world collision field");
    let Message::ImprovedTerseObjectUpdate(decoded) = decoded.message() else { panic!("expected terse object update") };
    assert!(decoded.objects[0].collision_plane.unwrap().x.is_nan());
}

#[test]
fn invalid_scale_preserves_sibling_blocks() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message().clone() else { panic!("expected full object update") };
    assert_eq!(update.objects.len(), 2);
    let mut bytes =
        Packet::new(packet.sequence(), Message::ObjectUpdate(update)).with_zero_coding(false).encode().expect("encode full update");
    let first_local_id = unique_offset(&bytes, &0x1020_3040_u32.to_le_bytes());
    write_nan(&mut bytes, first_local_id + 28);

    let decoded = Packet::decode(&bytes).expect("decode both framed object blocks");
    let Message::ObjectUpdate(decoded) = decoded.message() else { panic!("expected full object update") };
    assert_eq!(decoded.objects.len(), 2);
    assert!(decoded.objects[0].scale.x.is_nan());
    assert!(decoded.objects[1].scale.is_finite());
}

#[test]
fn unknown_wire_values_round_trip() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(mut update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    let object = &mut update.objects[0];
    object.pcode = u8::MAX;
    object.special_code = 0;
    object.angular_velocity = None;
    object.parent_local_id = 0;
    object.data = None;
    object.text = None;
    object.media_url = None;
    object.legacy_particle_system = None;
    object.extra_parameters.clear();
    object.sound = None;
    object.name_value = None;
    object.shape = None;
    object.texture_entry = None;
    object.texture_animation = None;
    object.particle_system = None;
    let encoded = Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update)).encode().expect("encode unknown PCode");
    let packet = Packet::decode(&encoded).expect("decode unknown PCode");
    let Message::ObjectUpdateCompressed(update) = packet.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects[0].pcode, u8::MAX);
    assert_eq!(packet.encode().expect("re-encode unknown PCode"), encoded);

    let (_, packet) = decode(vectors::REQUEST_MULTIPLE_OBJECTS);
    let Message::RequestMultipleObjects(request) = packet.message() else {
        panic!("expected full-data request");
    };
    let local_id = request.objects[0].local_id.to_le_bytes();
    let mut request_wire = fixture(vectors::REQUEST_MULTIPLE_OBJECTS);
    let local_id_offset = unique_offset(&request_wire, &local_id);
    request_wire[local_id_offset - 1] = u8::MAX;
    let packet = Packet::decode(&request_wire).expect("decode unknown cache-miss type");
    let Message::RequestMultipleObjects(request) = packet.message() else {
        panic!("expected full-data request");
    };
    assert_eq!(request.objects[0].cache_miss_type, CacheMissType::Unknown(u8::MAX));
    assert_reencodes(&packet, &request_wire);
}

#[test]
fn terse_rejects_undocumented_data_suffix() {
    let (_, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let mut wire = Packet::new(packet.sequence(), packet.message().clone()).with_zero_coding(false).encode().expect("encode terse fixture");
    let data_start = unique_offset(&wire, &0x1020_3040_u32.to_le_bytes());
    wire[data_start - 1] += 1;
    wire.insert(data_start + 44, 0xaa);
    assert_eq!(Packet::decode(&wire), Err(LLUdpError::InvalidMessage));
}

#[test]
fn terse_texture_entry_is_owned() {
    let (_, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(update) = packet.message().clone() else {
        panic!("expected terse object update");
    };
    let encoded = Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update)).encode().expect("encode texture entry");
    let decoded = Packet::decode(&encoded).expect("decode texture entry");
    let Message::ImprovedTerseObjectUpdate(update) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.objects[0].texture_entry, [0x11, 0, 0x22]);
}

#[test]
fn compressed_update_retains_unknown_flags() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(mut update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    update.objects[0].special_code |= 0x4000_0000;
    let encoded = Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update)).encode().expect("encode unknown flags");
    let decoded = Packet::decode(&encoded).expect("decode unknown packed flags");
    let Message::ObjectUpdateCompressed(update) = decoded.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects[0].special_code, 0x4000_07fe);
    assert_eq!(update.objects[0].angular_velocity, Some(Vec3::new(0.5, 0.0, -0.5)));
    assert_eq!(update.objects[0].parent_local_id, 0x0506_0708);
}

#[test]
fn compressed_optional_fields_can_be_absent() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(mut update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    update.objects[0].special_code = 0;
    update.objects[0].angular_velocity = None;
    update.objects[0].parent_local_id = 0;
    update.objects[0].data = None;
    update.objects[0].text = None;
    update.objects[0].media_url = None;
    update.objects[0].legacy_particle_system = None;
    update.objects[0].extra_parameters.clear();
    update.objects[0].sound = None;
    update.objects[0].name_value = None;
    update.objects[0].texture_animation = None;
    update.objects[0].particle_system = None;

    let encoded = Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update)).encode().expect("encode absent packed fields");
    let decoded = Packet::decode(&encoded).expect("decode absent packed fields");
    let Message::ObjectUpdateCompressed(update) = decoded.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects[0].angular_velocity, None);
    assert_eq!(update.objects[0].parent_local_id, 0);
    assert_eq!(update.objects[0].special_code, 0);
    assert_eq!(update.objects[0].text, None);
}

#[test]
fn extra_parameter_lengths_must_be_nonnegative() {
    let mut bytes = fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let offset = unique_offset(&bytes, &[0x34, 0x12, 0x03, 0x00, 0x00, 0x00]);
    bytes[offset + 2..offset + 6].copy_from_slice(&(-1i32).to_le_bytes());
    assert_eq!(Packet::decode(&bytes), Err(LLUdpError::InvalidMessage));
}

#[test]
fn typed_update_kinds_reach_consumer() {
    fn consume(packet: Packet) -> Option<&'static str> {
        match packet.into_message() {
            Message::ObjectUpdate(update) if !update.objects.is_empty() => Some("full"),
            Message::ObjectUpdateCompressed(update) if !update.objects.is_empty() => Some("full"),
            Message::ImprovedTerseObjectUpdate(update) if !update.objects.is_empty() => Some("partial"),
            Message::ObjectUpdateCached(update) if !update.objects.is_empty() => Some("cached"),
            Message::KillObject(update) if !update.object_data.is_empty() => Some("removed"),
            _ => None,
        }
    }

    for (wire, expected) in [
        (vectors::OBJECT_UPDATE, "full"),
        (vectors::OBJECT_UPDATE_COMPRESSED_TYPED, "full"),
        (vectors::IMPROVED_TERSE_OBJECT_UPDATE, "partial"),
        (vectors::OBJECT_UPDATE_CACHED, "cached"),
        (vectors::KILL_OBJECT, "removed"),
    ] {
        let (_, packet) = decode(wire);
        assert_eq!(consume(packet), Some(expected));
    }
}
