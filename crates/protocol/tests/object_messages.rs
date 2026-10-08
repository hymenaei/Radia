use base::UUID;
use protocol::{
    UdpError,
    messages::{
        CacheMissType, CachedObjectReference, KillObject, ObjectRegionData, ObjectRequest, ObjectRotation, ObjectUpdateCached,
        RequestAgentData, RequestMultipleObjects,
    },
    udp::{Message, Packet},
};
use test_support::{parse_hex_fixture, udp::vectors};

fn fixture(hex: &str) -> Vec<u8> {
    parse_hex_fixture(hex).expect("valid protocol fixture")
}

fn unique_offset(bytes: &[u8], needle: &[u8]) -> usize {
    let mut offsets = bytes.windows(needle.len()).enumerate().filter_map(|(offset, candidate)| (candidate == needle).then_some(offset));
    let offset = offsets.next().expect("fixture field is present");
    assert!(offsets.next().is_none(), "fixture field is unique");
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
    assert_eq!(update.region_data, ObjectRegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 });
    assert_eq!(update.objects.len(), 2);
    assert_eq!(update.objects[0].local_id, 0x1020_3040);
    assert_eq!(update.objects[0].id, uuid("12345678-90ab-cdef-0123-456789abcdef"));
    assert_eq!(update.objects[0].avatar_collision_plane, None);
    assert!(update.objects[0].opaque_motion_data.is_empty());
    assert!(!update.objects[0].opaque_data.is_empty());
    assert_eq!(update.objects[1].avatar_collision_plane, Some([0.0, 0.0, 1.0, 0.0]));
    assert!(update.objects[1].opaque_motion_data.is_empty());
    assert_reencodes(&packet, &bytes);
}

#[test]
fn full_update_extended_motion_is_preserved() {
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
        assert_eq!(extended_object.avatar_collision_plane, object.avatar_collision_plane);
        assert_eq!(extended_object.opaque_motion_data, extension);
        assert_eq!(extended_object.opaque_data, object.opaque_data);
        assert_reencodes(&extended_packet, &extended_wire);
    }
}

#[test]
fn full_update_rejects_opaque_motion_lengths_that_change_the_layout() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected full object update");
    };

    update.objects[0].opaque_motion_data.resize(16, 0xaa);
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdate(update)).encode(), Err(UdpError::InvalidMessage));

    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    update.objects[1].opaque_motion_data.resize(48, 0xaa);
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdate(update)).encode(), Err(UdpError::InvalidMessage));
}

#[test]
fn compressed_update_fixture() {
    let (bytes, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(update) = packet.message() else {
        panic!("expected compressed full object update");
    };

    assert_eq!(update.objects.len(), 1);
    let object = &update.objects[0];
    assert_eq!(update.region_data.time_dilation, 0x8000);
    assert_eq!(object.local_id, 0x0102_0304);
    assert_eq!(object.id, uuid("12345678-90ab-cdef-0123-456789abcdef"));
    assert_eq!(object.angular_velocity, Some(protocol::Vector3 { x: 0.5, y: 0.0, z: -0.5 }));
    assert_eq!(object.parent_local_id, Some(0x0506_0708));
    assert_eq!(object.opaque_data, [0xde, 0xad, 0, 0xbe, 0xef]);
    assert_reencodes(&packet, &bytes);
}

#[test]
fn terse_update_fixtures_are_partial() {
    let (bytes, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(update) = packet.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.objects.len(), 1);
    assert_eq!(update.objects[0].local_id, 0x1020_3040);
    assert_eq!(update.objects[0].avatar_collision_plane, None);
    assert_eq!(update.objects[0].texture_entry.as_deref(), Some(&[0x11, 0, 0x22][..]));
    let canonical = packet.encode().expect("re-encode terse object update");
    assert_ne!(canonical, bytes, "zero snap canonicalizes the near-zero quantized value");
    assert_eq!(Packet::decode(&canonical).expect("decode canonical terse update").message(), packet.message());

    let (avatar_bytes, avatar_packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let Message::ImprovedTerseObjectUpdate(avatar_update) = avatar_packet.message() else {
        panic!("expected terse avatar update");
    };
    assert_eq!(avatar_update.objects[0].agent_data_flag, 1);
    assert_eq!(avatar_update.objects[0].avatar_collision_plane, Some([0.0, 0.0, 1.0, 0.0]));
    assert_reencodes(&avatar_packet, &avatar_bytes);
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
    assert_eq!(motion.velocity, protocol::Vector3::default());
    assert_eq!(motion.acceleration, protocol::Vector3::default());
    assert_eq!(motion.rotation, ObjectRotation::default());
    assert_eq!(motion.angular_velocity, protocol::Vector3::default());

    let encoded = Packet::new(packet.sequence(), packet.message().clone()).encode().expect("re-encode zero motion");
    let decoded = Packet::decode(&encoded).expect("decode re-encoded zero motion");
    let Message::ImprovedTerseObjectUpdate(update) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert_eq!(update.objects[0].motion.velocity, protocol::Vector3::default());
    assert_eq!(update.objects[0].motion.acceleration, protocol::Vector3::default());
    assert_eq!(update.objects[0].motion.rotation, ObjectRotation::default());
    assert_eq!(update.objects[0].motion.angular_velocity, protocol::Vector3::default());
}

#[test]
fn cached_and_kill_fixtures() {
    let (cache_bytes, cache_packet) = decode(vectors::OBJECT_UPDATE_CACHED);
    assert_eq!(
        cache_packet.message(),
        &Message::ObjectUpdateCached(ObjectUpdateCached {
            region_data: ObjectRegionData { region_handle: 0x1122_3344_5566_7788, time_dilation: 0x8000 },
            objects: vec![
                CachedObjectReference { local_id: 0x0102_0304, crc: 0x1122_3344, update_flags: 0x5566_7788 },
                CachedObjectReference { local_id: 0x1020_3040, crc: 0xaabb_ccdd, update_flags: 0x8765_4321 },
            ],
        })
    );
    assert_reencodes(&cache_packet, &cache_bytes);

    let (kill_bytes, kill_packet) = decode(vectors::KILL_OBJECT);
    assert_eq!(kill_packet.message(), &Message::KillObject(KillObject { local_ids: vec![0x0102_0304, 0x5566_7788] }));
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
            update.local_ids.clear();
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
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdate(update)).encode(), Err(UdpError::InvalidMessage));

    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    update.objects[0].opaque_motion_data.push(0xaa);
    assert_eq!(Packet::new(packet.sequence(), Message::ObjectUpdate(update)).encode(), Err(UdpError::InvalidMessage));

    let (_, packet) = decode(vectors::OBJECT_UPDATE);
    let Message::ObjectUpdate(update) = packet.message().clone() else {
        panic!("expected full object update");
    };
    let mut malformed =
        Packet::new(packet.sequence(), Message::ObjectUpdate(update)).with_zero_coding(false).encode().expect("encode full update");
    malformed[58] = 61;
    assert_eq!(Packet::decode(&malformed), Err(UdpError::InvalidMessage));

    let mut truncated = fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    truncated[22..24].copy_from_slice(&84u16.to_le_bytes());
    truncated.truncate(24 + 84);
    assert_eq!(Packet::decode(&truncated), Err(UdpError::TruncatedPacket));
}

#[test]
fn decoder_rejects_non_finite_spatial_values() {
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
    assert_eq!(Packet::decode(&full), Err(UdpError::InvalidMessage));

    let mut compressed = fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let local_id = 0x0102_0304_u32.to_le_bytes();
    let local_id_offset = unique_offset(&compressed, &local_id);
    write_nan(&mut compressed, local_id_offset + 24);
    assert_eq!(Packet::decode(&compressed), Err(UdpError::InvalidMessage));

    let mut terse = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let local_id = 0x1020_3040_u32.to_le_bytes();
    let local_id_offset = unique_offset(&terse, &local_id);
    write_nan(&mut terse, local_id_offset + 6);
    assert_eq!(Packet::decode(&terse), Err(UdpError::InvalidMessage));

    let (_, avatar_packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let Message::ImprovedTerseObjectUpdate(avatar_update) = avatar_packet.message() else {
        panic!("expected terse avatar update");
    };
    let local_id = avatar_update.objects[0].local_id.to_le_bytes();
    let mut avatar = fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR);
    let local_id_offset =
        avatar.windows(local_id.len()).rposition(|candidate| candidate == local_id.as_slice()).expect("avatar local ID is present");
    write_nan(&mut avatar, local_id_offset + 6);
    assert_eq!(Packet::decode(&avatar), Err(UdpError::InvalidMessage));
}

#[test]
fn unknown_wire_values_round_trip() {
    let mut compressed = fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let local_id = 0x0102_0304_u32.to_le_bytes();
    let local_id_offset = unique_offset(&compressed, &local_id);
    compressed[local_id_offset + 4] = u8::MAX;
    let packet = Packet::decode(&compressed).expect("decode unknown PCode");
    let Message::ObjectUpdateCompressed(update) = packet.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects[0].pcode, u8::MAX);
    assert_reencodes(&packet, &compressed);

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
fn terse_suffix_and_unknown_compressed_flags_are_owned() {
    let (_, packet) = decode(vectors::IMPROVED_TERSE_OBJECT_UPDATE);
    let Message::ImprovedTerseObjectUpdate(mut update) = packet.message().clone() else {
        panic!("expected terse object update");
    };
    update.objects[0].opaque_data.extend_from_slice(&[0xaa, 0xbb]);
    let encoded = Packet::new(packet.sequence(), Message::ImprovedTerseObjectUpdate(update)).encode().expect("encode opaque suffix");
    let decoded = Packet::decode(&encoded).expect("decode opaque suffix");
    let Message::ImprovedTerseObjectUpdate(update) = decoded.message() else {
        panic!("expected terse object update");
    };
    assert!(update.objects[0].opaque_data.ends_with(&[0xaa, 0xbb]));

    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(mut update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    update.objects[0].special_flags = 0x4000_00a0;
    let encoded = Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update)).encode().expect("encode unknown flags");
    let decoded = Packet::decode(&encoded).expect("decode unknown packed flags");
    let Message::ObjectUpdateCompressed(update) = decoded.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects[0].special_flags, 0x4000_00a0);
    assert_eq!(update.objects[0].angular_velocity, Some(protocol::Vector3 { x: 0.5, y: 0.0, z: -0.5 }));
    assert_eq!(update.objects[0].parent_local_id, Some(0x0506_0708));
}

#[test]
fn optional_packed_fields_can_be_absent() {
    let (_, packet) = decode(vectors::OBJECT_UPDATE_COMPRESSED_TYPED);
    let Message::ObjectUpdateCompressed(mut update) = packet.message().clone() else {
        panic!("expected compressed full object update");
    };
    update.objects[0].special_flags = 0;
    update.objects[0].angular_velocity = None;
    update.objects[0].parent_local_id = None;
    update.objects[0].opaque_data.clear();

    let encoded = Packet::new(packet.sequence(), Message::ObjectUpdateCompressed(update)).encode().expect("encode absent packed fields");
    let decoded = Packet::decode(&encoded).expect("decode absent packed fields");
    let Message::ObjectUpdateCompressed(update) = decoded.message() else {
        panic!("expected compressed full object update");
    };
    assert_eq!(update.objects[0].angular_velocity, None);
    assert_eq!(update.objects[0].parent_local_id, None);
}

#[test]
fn typed_update_kinds_reach_consumer() {
    fn consume(packet: Packet) -> Option<&'static str> {
        match packet.into_message() {
            Message::ObjectUpdate(update) if !update.objects.is_empty() => Some("full"),
            Message::ObjectUpdateCompressed(update) if !update.objects.is_empty() => Some("full"),
            Message::ImprovedTerseObjectUpdate(update) if !update.objects.is_empty() => Some("partial"),
            Message::ObjectUpdateCached(update) if !update.objects.is_empty() => Some("cached"),
            Message::KillObject(update) if !update.local_ids.is_empty() => Some("removed"),
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
