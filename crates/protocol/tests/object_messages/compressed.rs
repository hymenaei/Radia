use super::*;

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
