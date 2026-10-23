use super::*;

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
        vectors::OBJECT_EXTRA_PARAMS,
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
    assert!(terse_object(&decoded.objects[0]).motion.position.x.is_nan());

    let (_, avatar_packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let Message::ImprovedTerseObjectUpdate(avatar_update) = avatar_packet.message() else {
        panic!("expected terse avatar update");
    };
    let local_id = terse_object(&avatar_update.objects[0]).local_id.to_le_bytes();
    let mut avatar = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let local_id_offset =
        avatar.windows(local_id.len()).rposition(|candidate| candidate == local_id.as_slice()).expect("avatar local ID is present");
    write_nan(&mut avatar, local_id_offset + 6);
    let decoded = Packet::decode(&avatar).expect("decode avatar update with a non-world collision field");
    let Message::ImprovedTerseObjectUpdate(decoded) = decoded.message() else { panic!("expected terse object update") };
    assert!(terse_object(&decoded.objects[0]).collision_plane.unwrap().x.is_nan());
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
