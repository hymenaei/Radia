use std::assert_matches;

use super::*;

#[test]
fn terse_update_fixtures_are_partial() {
    let (bytes, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.region_data, RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(update.objects.len(), 1);
    let object = terse_object(&update.objects[0]);
    assert_eq!(object.local_id, 0x1020_3040);
    assert_eq!(object.state, 5);
    assert!(!object.avatar);
    assert_eq!(object.collision_plane, None);
    assert_eq!(object.motion.position, Vec3::new(5.0, 6.0, 7.0));
    assert_eq!(object.motion.velocity, Vec3::new(-128.0, 0.0, 128.0));
    assert_eq!(object.motion.acceleration, Vec3::new(-64.0, 0.0, 64.0));
    assert_eq!(object.motion.rotation, Quat::from_xyzw(-1.0, 0.0, 1.0, 0.0));
    assert_eq!(object.motion.angular_velocity, Vec3::new(-64.0, 0.0, 64.0));
    assert_eq!(object.texture_entry.as_slice(), &[0x11, 0, 0x22]);
    let canonical = packet.encode().expect("re-encode terse object update");
    assert_ne!(canonical, bytes, "zero snap canonicalizes the near-zero quantized value");
    assert_eq!(Packet::decode(&canonical).expect("decode canonical terse update").message(), packet.message());

    let (avatar_bytes, avatar_packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let Message::ImprovedTerseObjectUpdate(avatar_update) = avatar_packet.message() else {
        panic!("expected terse avatar update");
    };
    assert_eq!(avatar_update.region_data, RegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(avatar_update.objects.len(), 1);
    let avatar = terse_object(&avatar_update.objects[0]);
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
fn terse_avatar_payload_constraints() {
    let (_, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected terse object update");
    };
    terse_object_mut(&mut update.objects[0]).texture_entry.clear();
    let encoded =
        Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update.clone())).encode().expect("encode empty texture entry");
    let decoded = Packet::decode(&encoded).expect("decode empty texture entry");
    let Message::ImprovedTerseObjectUpdate(decoded) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert!(terse_object(&decoded.objects[0]).texture_entry.is_empty());

    terse_object_mut(&mut update.objects[0]).avatar = true;
    assert_eq!(
        Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update.clone())).encode(),
        Err(LLUdpError::InvalidMessage),
    );

    terse_object_mut(&mut update.objects[0]).avatar = false;
    terse_object_mut(&mut update.objects[0]).collision_plane = Some(Vec4::ZERO);
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
    let motion = &terse_object(&update.objects[0]).motion;
    assert_eq!(motion.velocity, Vec3::ZERO);
    assert_eq!(motion.acceleration, Vec3::ZERO);
    assert_eq!(motion.rotation, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
    assert_eq!(motion.angular_velocity, Vec3::ZERO);

    let encoded = Packet::new(packet.sequence(), packet.message().clone()).encode().expect("re-encode zero motion");
    let decoded = Packet::decode(&encoded).expect("decode re-encoded zero motion");
    let Message::ImprovedTerseObjectUpdate(update) = decoded.message() else {
        panic!("expected terse object update");
    };
    let object = terse_object(&update.objects[0]);
    assert_eq!(object.motion.velocity, Vec3::ZERO);
    assert_eq!(object.motion.acceleration, Vec3::ZERO);
    assert_eq!(object.motion.rotation, Quat::from_xyzw(0.0, 0.0, 0.0, 0.0));
    assert_eq!(object.motion.angular_velocity, Vec3::ZERO);
}

#[test]
fn terse_ignores_unknown_data_suffix() {
    let mut wire = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let data_start = unique_offset(&wire, &0x1020_3040_u32.to_le_bytes());
    wire[data_start - 1] += 1;
    wire.insert(data_start + 44, 0xaa);
    let packet = Packet::decode(&wire).expect("ignore unknown packed-data suffix");
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.objects.len(), 1);
    let object = terse_object(&update.objects[0]);
    assert_eq!(object.local_id, 0x1020_3040);
    assert_eq!(object.motion.position, Vec3::new(5.0, 6.0, 7.0));
}

#[test]
fn malformed_terse_block_preserves_later_blocks() {
    let original = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let data_start = unique_offset(&original, &0x1020_3040_u32.to_le_bytes());
    let packed_data = &original[data_start..data_start + 44];
    let texture_entry = &original[data_start + 46..data_start + 49];

    let mut wire = original[..18].to_vec();
    wire[17] = 2;
    wire.push(5);
    wire.extend_from_slice(&packed_data[..5]);
    wire.extend_from_slice(&0_u16.to_le_bytes());
    wire.push(44);
    wire.extend_from_slice(packed_data);
    wire.extend_from_slice(&3_u16.to_le_bytes());
    wire.extend_from_slice(texture_entry);

    let packet = Packet::decode(&wire).expect("malformed framed block is isolated");
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    assert_matches!(
        &update.objects[0],
        improved_terse_object_update::ObjectBlock::Malformed { packed_data, texture_entry }
            if packed_data.as_slice() == &original[data_start..data_start + 5] && texture_entry.is_empty()
    );
    assert_eq!(terse_object(&update.objects[1]).local_id, 0x1020_3040);

    let encoded = packet.encode().expect("malformed block remains encodable");
    let decoded = Packet::decode(&encoded).expect("redecode preserved blocks");
    let Message::ImprovedTerseObjectUpdate(decoded) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert_matches!(&decoded.objects[0], improved_terse_object_update::ObjectBlock::Malformed { .. });
    assert_eq!(terse_object(&decoded.objects[1]).local_id, 0x1020_3040);
}

#[test]
fn missing_terse_collision_plane_preserves_later_blocks() {
    let original = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let data_start = unique_offset(&original, &0x1020_3040_u32.to_le_bytes());
    let mut packed_data = original[data_start..data_start + 44].to_vec();
    packed_data[5] = 1;
    let texture_entry = &original[data_start + 46..data_start + 49];

    let mut wire = original[..18].to_vec();
    wire[17] = 2;
    wire.push(44);
    wire.extend_from_slice(&packed_data);
    wire.extend_from_slice(&0_u16.to_le_bytes());
    wire.push(44);
    wire.extend_from_slice(&original[data_start..data_start + 44]);
    wire.extend_from_slice(&3_u16.to_le_bytes());
    wire.extend_from_slice(texture_entry);

    let preflight = Packet::preflight(&wire).expect("a malformed typed block remains framed");
    let packet = Packet::decode(&wire).expect("a malformed typed block is isolated");
    assert!(preflight.retained_capacity_upper_bound() >= packet.retained_capacity_bytes());
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    assert_matches!(
        &update.objects[0],
        improved_terse_object_update::ObjectBlock::Malformed { packed_data: actual, texture_entry }
            if actual == &packed_data && texture_entry.is_empty()
    );
    assert_eq!(terse_object(&update.objects[1]).local_id, 0x1020_3040);
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
    assert_eq!(terse_object(&update.objects[0]).texture_entry, [0x11, 0, 0x22]);
}
