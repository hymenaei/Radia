use base::UUID;
use math::{Quat, Vec3};
use protocol::{
    Audible, BlockInstance, BlockValues, ChatSourceType, ChatType, FieldValue, Frequency, LLUdpError, SchemaMessage,
    lludp::{Message, Packet},
    messages::{
        AgentMovementComplete, AgentUpdate, ChatFromSimulator, ChatFromViewer, ConfirmEnableSimulator, DisableSimulator, EnableSimulator,
        LogoutReply,
        agent_movement_complete::{AgentData, Data, SimData},
        agent_update,
        chat_from_simulator::ChatData,
        chat_from_viewer, confirm_enable_simulator,
        enable_simulator::SimulatorInfo,
        logout_reply,
        logout_reply::InventoryData,
    },
    schema::{MessageEncoding, MessageTrust},
};
use std::net::Ipv4Addr;
use test_support::{lludp::vectors, parse_hex_fixture};

fn fixture(hex: &str) -> Vec<u8> {
    parse_hex_fixture(hex).expect("valid protocol fixture")
}

fn session_id() -> UUID {
    UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("valid fixture UUID")
}

fn agent_id() -> UUID {
    UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("valid fixture UUID")
}

#[test]
fn movement_complete_fields() {
    let packet_bytes = fixture(vectors::AGENT_MOVEMENT_COMPLETE);
    let decoded = Packet::decode(&packet_bytes).expect("decode fixture");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(
        decoded.message(),
        &Message::AgentMovementComplete(AgentMovementComplete {
            agent_data: AgentData { agent_id: agent_id(), session_id: session_id() },
            data: Data {
                position: Vec3::new(128.25, 64.5, 23.0),
                look_at: Vec3::new(0.0, 1.0, 0.0),
                region_handle: 0x1122_3344_5566_7788,
                timestamp: 0x0102_0304,
            },
            sim_data: SimData { channel_version: "test-1".into() },
        })
    );
    assert_eq!(decoded.encode().expect("re-encode movement fixture"), packet_bytes);
}

#[test]
fn agent_update_quaternions_use_canonical_xyz() {
    fn message(rotation: Quat) -> Message {
        Message::AgentUpdate(AgentUpdate {
            agent_data: agent_update::AgentData {
                agent_id: agent_id(),
                session_id: session_id(),
                body_rotation: rotation,
                head_rotation: rotation,
                state: 0,
                camera_center: Vec3::ZERO,
                camera_at_axis: Vec3::ZERO,
                camera_left_axis: Vec3::ZERO,
                camera_up_axis: Vec3::ZERO,
                far: 0.0,
                control_flags: 0,
                flags: 0,
            },
        })
    }

    let rotation = Quat::from_xyzw(0.2, -0.3, 0.4, -0.84);
    let equivalent = Quat::from_xyzw(-rotation.x, -rotation.y, -rotation.z, -rotation.w);
    let encoded = Packet::new(1, message(rotation)).encode().expect("encode negative-W rotation");
    let encoded_equivalent = Packet::new(1, message(equivalent)).encode().expect("encode equivalent rotation");
    assert_eq!(encoded, encoded_equivalent);

    let decoded = Packet::decode(&encoded).expect("decode canonical rotation");
    let Message::AgentUpdate(decoded) = decoded.message() else {
        panic!("expected AgentUpdate");
    };
    for decoded_rotation in [decoded.agent_data.body_rotation, decoded.agent_data.head_rotation] {
        assert!(decoded_rotation.w >= 0.0);
        assert!((rotation.normalize().dot(decoded_rotation.normalize()).abs() - 1.0).abs() < 1.0e-5);
    }

    assert_eq!(Packet::new(1, message(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0))).encode(), Err(LLUdpError::InvalidMessage));
}

#[test]
fn logout_reply_fields() {
    let expected = fixture(vectors::LOGOUT_REPLY);
    let decoded = Packet::decode(&expected).expect("decode fixture");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert!(decoded.is_reliable());
    assert_eq!(
        decoded.message(),
        &Message::LogoutReply(LogoutReply {
            agent_data: logout_reply::AgentData { agent_id: agent_id(), session_id: session_id() },
            inventory_data: vec![InventoryData {
                item_id: UUID::parse_str("12345678-90ab-cdef-0123-456789abcdef").expect("valid item UUID"),
            }],
        })
    );
    let encoded = decoded.encode().expect("encode decoded logout reply");
    assert_eq!(encoded, expected);
    assert_eq!(Packet::decode(&encoded).expect("decode encoded logout reply").message(), decoded.message());

    let mut truncated = expected;
    truncated.pop();
    assert!(matches!(Packet::decode(&truncated), Err(LLUdpError::TruncatedPacket)));
}

#[test]
fn enable_simulator_fields() {
    let expected = fixture(vectors::ENABLE_SIMULATOR);
    let decoded = Packet::decode(&expected).expect("decode EnableSimulator fixture");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(
        decoded.message(),
        &Message::EnableSimulator(EnableSimulator {
            simulator_info: SimulatorInfo { handle: 0x1122_3344_5566_7788, ip: Ipv4Addr::new(192, 0, 2, 10), port: 0x32c8 },
        })
    );
    assert!(decoded.message().is_inbound_only());
    assert_eq!(decoded.encode().expect("re-encode EnableSimulator fixture"), expected);
}

#[test]
fn disable_simulator_fields() {
    let expected = fixture(vectors::DISABLE_SIMULATOR);
    let decoded = Packet::decode(&expected).expect("decode DisableSimulator fixture");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(decoded.message(), &Message::DisableSimulator(DisableSimulator));
    assert!(decoded.message().is_inbound_only());
    assert_eq!(decoded.encode().expect("re-encode DisableSimulator fixture"), expected);
}

#[test]
fn confirm_enable_simulator_fields() {
    let expected = fixture(vectors::CONFIRM_ENABLE_SIMULATOR);
    let decoded = Packet::decode(&expected).expect("decode ConfirmEnableSimulator fixture");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(
        decoded.message(),
        &Message::ConfirmEnableSimulator(ConfirmEnableSimulator {
            agent_data: confirm_enable_simulator::AgentData { agent_id: agent_id(), session_id: session_id() },
        })
    );
    assert_eq!(decoded.encode().expect("re-encode ConfirmEnableSimulator fixture"), expected);
}

#[test]
fn confirm_enable_simulator_is_inbound_only() {
    let message = Message::ConfirmEnableSimulator(ConfirmEnableSimulator { agent_data: confirm_enable_simulator::AgentData::default() });

    assert!(message.is_inbound_only());
}

#[test]
fn chat_from_simulator_fields() {
    let expected = fixture(vectors::CHAT_FROM_SIMULATOR);
    let decoded = Packet::decode(&expected).expect("decode fixture");

    assert!(decoded.trailing_message_bytes().is_empty(), "fixture has unparsed message bytes");
    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(
        decoded.message(),
        &Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: "Ada".into(),
                source_id: UUID::parse_str("11223344-5566-7788-99aa-bbccddeeff00").expect("valid source UUID"),
                owner_id: agent_id(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Shout,
                audible: Audible::Fully,
                position: Vec3::new(1.0, -2.5, 0.25),
                message: "Hi".into(),
            },
        })
    );
    assert_eq!(decoded.encode().expect("re-encode chat fixture"), expected);
}

#[test]
fn invalid_utf8_message() {
    let expected = fixture(vectors::CHAT_FROM_SIMULATOR_INVALID_UTF8_MESSAGE);
    assert_eq!(Packet::decode(&expected), Err(LLUdpError::InvalidUtf8));
}

#[test]
fn invalid_utf8_name() {
    let expected = fixture(vectors::CHAT_FROM_SIMULATOR_INVALID_UTF8_NAME);
    assert_eq!(Packet::decode(&expected), Err(LLUdpError::InvalidUtf8));
}

#[test]
fn chat_utf8_lengths() {
    let expected = fixture(vectors::CHAT_FROM_SIMULATOR_UTF8);
    let decoded = Packet::decode(&expected).expect("decode multibyte chat fixture");
    let Message::ChatFromSimulator(message) = decoded.message() else {
        panic!("expected ChatFromSimulator");
    };

    assert_eq!(message.chat_data.from_name, "Åda");
    assert_eq!(message.chat_data.message, "世界");
    assert!(!message.chat_data.from_name.contains('\0'));
    assert!(!message.chat_data.message.contains('\0'));
    assert_eq!(decoded.encode().expect("re-encode multibyte chat fixture"), expected);
}

#[test]
fn chat_nul_terminates_strings() {
    let bytes = fixture(vectors::CHAT_FROM_SIMULATOR_EMBEDDED_NUL);
    let decoded = Packet::decode(&bytes).expect("decode embedded-NUL chat fixture");
    let Message::ChatFromSimulator(message) = decoded.message() else {
        panic!("expected ChatFromSimulator");
    };

    assert_eq!(message.chat_data.from_name, "A");
    assert_eq!(message.chat_data.message, "m");
}

#[test]
fn outbound_strings_reject_nul() {
    let messages = [
        Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: "A\0B".into(),
                source_id: UUID::nil(),
                owner_id: UUID::nil(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vec3::ZERO,
                message: "text".into(),
            },
        }),
        Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: "name".into(),
                source_id: UUID::nil(),
                owner_id: UUID::nil(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vec3::ZERO,
                message: "A\0B".into(),
            },
        }),
        Message::ChatFromViewer(ChatFromViewer {
            agent_data: chat_from_viewer::AgentData { agent_id: UUID::nil(), session_id: UUID::nil() },
            chat_data: chat_from_viewer::ChatData { message: "A\0B".into(), chat_type: ChatType::Normal, channel: 0 },
        }),
    ];

    for message in messages {
        assert_eq!(Packet::new(1, message).encode(), Err(LLUdpError::InvalidMessage));
    }
}

#[test]
fn inaudible_empty_chat() {
    let expected = fixture(vectors::CHAT_FROM_SIMULATOR_INAUDIBLE_EMPTY);
    let decoded = Packet::decode(&expected).expect("decode fixture");

    assert!(decoded.trailing_message_bytes().is_empty());
    assert_eq!(
        decoded.message(),
        &Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: "None".into(),
                source_id: UUID::nil(),
                owner_id: UUID::nil(),
                source_type: ChatSourceType::Object,
                chat_type: ChatType::Normal,
                audible: Audible::Barely,
                position: Vec3::ZERO,
                message: String::new(),
            },
        })
    );
    assert_eq!(decoded.encode().expect("re-encode inaudible chat fixture"), expected);
}

#[test]
fn unknown_chat_round_trip() {
    for (wire, expected) in [
        (0, ChatType::Whisper),
        (1, ChatType::Normal),
        (2, ChatType::Shout),
        (3, ChatType::Say),
        (4, ChatType::StartTyping),
        (5, ChatType::StopTyping),
        (6, ChatType::Debug),
        (8, ChatType::OwnerSay),
    ] {
        assert_eq!(ChatType::from_wire(wire), expected);
        assert_eq!(expected.to_wire(), wire);
    }
    assert_eq!(ChatType::from_wire(7), ChatType::Unknown(7));
    assert_eq!(ChatType::Unknown(7).to_wire(), 7);

    for (wire, expected) in [(0, ChatSourceType::System), (1, ChatSourceType::Agent), (2, ChatSourceType::Object)] {
        assert_eq!(ChatSourceType::from_wire(wire), expected);
        assert_eq!(expected.to_wire(), wire);
    }
    assert_eq!(ChatSourceType::from_wire(3), ChatSourceType::Unknown(3));
    assert_eq!(ChatSourceType::Unknown(3).to_wire(), 3);

    assert_eq!(Audible::from_wire(u8::MAX), Audible::Not);
    assert_eq!(Audible::Not.to_wire(), u8::MAX);
    assert_eq!(Audible::from_wire(2), Audible::Unknown(2));
    assert_eq!(Audible::Unknown(2).to_wire(), 2);
}

#[test]
fn coarse_location_fixture() {
    let expected = fixture(vectors::COARSE_LOCATION_UPDATE);
    let decoded = Packet::decode(&expected).expect("decode independently assembled schema fixture");
    let agent_ids = BlockValues::new(vec![
        BlockInstance::new(vec![FieldValue::UUID(UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("valid fixture UUID"))]),
        BlockInstance::new(vec![FieldValue::UUID(UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("valid fixture UUID"))]),
    ]);
    let schema = SchemaMessage::schema_by_name("CoarseLocationUpdate").expect("generated message schema");
    let expected_message = SchemaMessage::new(
        schema,
        vec![
            BlockValues::new(vec![
                BlockInstance::new(vec![FieldValue::U8(0x11), FieldValue::U8(0x22), FieldValue::U8(0x33)]),
                BlockInstance::new(vec![FieldValue::U8(0x44), FieldValue::U8(0x55), FieldValue::U8(0x66)]),
            ]),
            BlockValues::single(vec![FieldValue::S16(-2), FieldValue::S16(0x1234)]),
            agent_ids,
        ],
    )
    .expect("valid schema message");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(decoded.message(), &Message::Schema(expected_message));
    assert_eq!(decoded.encode().expect("re-encode schema fixture"), expected);
}

#[test]
fn compressed_update_requires_packed_identity() {
    let packet = fixture(vectors::OBJECT_UPDATE_COMPRESSED);
    assert_eq!(Packet::decode(&packet), Err(LLUdpError::TruncatedPacket));

    let schema = SchemaMessage::schema_by_name("ObjectUpdateCompressed").expect("generated schema remains available");
    assert_eq!(schema.id.frequency, Frequency::High);
    assert_eq!(schema.id.number, 13);
    assert_eq!(schema.trust, MessageTrust::Trusted);
    assert_eq!(schema.encoding, MessageEncoding::Unencoded);
    assert_eq!(schema.blocks.len(), 2);
}

#[test]
fn empty_sole_variable_block() {
    let expected = fixture(vectors::UUID_NAME_REQUEST_EMPTY);
    let decoded = Packet::decode(&expected).expect("decode legacy empty UUIDNameRequest");

    assert_eq!(decoded.sequence(), 1);
    let Message::Schema(message) = decoded.message() else {
        panic!("expected schema-backed UUIDNameRequest");
    };
    assert_eq!(message.schema().name, "UUIDNameRequest");
    assert_eq!(message.blocks().len(), 1);
    assert!(message.blocks()[0].instances.is_empty());
    assert!(message.trailing_bytes().is_empty());
    assert_eq!(decoded.encode().expect("re-encode legacy UUIDNameRequest"), expected);
}
