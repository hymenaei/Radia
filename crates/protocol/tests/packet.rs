use base::UUID;
use protocol::{
    BlockInstance, BlockValues, ChatType, FieldValue, LLUdpError, SchemaMessage,
    lludp::{Message, MessageId, Packet, PacketFlags},
    messages::{
        ChatFromViewer, CompleteAgentMovement, CompletePingCheck, LogoutReply, LogoutRequest, MapBlockReply, MapBlockRequest, PacketAck,
        PingId, RegionHandleRequest, RegionHandshakeReply, RegionIdAndHandleReply, StartPingCheck, UseCircuitCode,
        chat_from_viewer::{AgentData, ChatData},
        complete_agent_movement,
        logout_reply::{self, InventoryData},
        logout_request, map_block_reply,
        map_block_request::{self, PositionData},
        packet_ack::Packets,
        region_handle_request::RequestBlock,
        region_handshake_reply,
        region_id_and_handle_reply::ReplyBlock,
        use_circuit_code::CircuitCode,
    },
    schema::MessageEncoding,
};
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

fn chat_message(message: String, chat_type: ChatType, channel: i32) -> Message {
    Message::ChatFromViewer(ChatFromViewer {
        agent_data: AgentData { agent_id: agent_id(), session_id: session_id() },
        chat_data: ChatData { message, chat_type, channel },
    })
}

fn complete_ping(ping_id: u8) -> Message {
    Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id } })
}

fn start_ping(ping_id: u8, oldest_unacked: u32) -> Message {
    Message::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id }, oldest_unacked })
}

fn packet_ack(packet_ids: &[u32]) -> Message {
    Message::PacketAck(PacketAck { packets: packet_ids.iter().map(|id| Packets { id: *id }).collect() })
}

#[test]
fn circuit_code_round_trip() {
    let expected = fixture(vectors::USE_CIRCUIT_CODE);
    let message = Message::UseCircuitCode(UseCircuitCode {
        circuit_code: CircuitCode { code: 0x1122_3344, session_id: session_id(), agent_id: agent_id() },
    });
    let packet = Packet::new(0x0102_0304, message.clone()).with_reliable();

    assert_eq!(packet.encode().expect("encode packet"), expected);
    let decoded = Packet::decode(&expected).expect("decode fixture");
    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn replacement_preserves_header() {
    let source = fixture(vectors::REGION_HANDSHAKE);
    let packet = Packet::decode(&source).expect("decode source packet");
    let flags = packet.header().expect("source header").flags();
    assert!(flags.contains(PacketFlags::ZEROCODED));

    let encoded = packet.with_message(complete_ping(1)).encode().expect("encode replacement message");
    let replaced = Packet::decode(&encoded).expect("decode replacement packet");
    assert_eq!(replaced.sequence(), 0x0102_0304);
    assert_eq!(replaced.header().expect("replacement header").flags(), flags);
    assert_eq!(replaced.message(), &complete_ping(1));
}

#[test]
fn replacement_handles_trailing_bytes() {
    let mut source = fixture(vectors::START_PING_CHECK);
    source.extend_from_slice(&[0xde, 0xad]);
    let packet = Packet::decode(&source).expect("decode packet with an extension");
    assert_eq!(packet.trailing_message_bytes(), [0xde, 0xad]);

    let same_id = packet.clone().with_message(start_ping(2, 0x0102_0304)).encode().expect("encode same-ID replacement");
    let same_id = Packet::decode(&same_id).expect("decode same-ID replacement");
    assert_eq!(same_id.trailing_message_bytes(), [0xde, 0xad]);

    let different_id = packet.with_message(complete_ping(1)).encode().expect("encode different-ID replacement");
    let different_id = Packet::decode(&different_id).expect("decode different-ID replacement");
    assert!(different_id.trailing_message_bytes().is_empty());
}

#[test]
fn handshake_reply_bytes() {
    let expected = fixture(vectors::REGION_HANDSHAKE_REPLY);
    let message = Message::RegionHandshakeReply(RegionHandshakeReply {
        agent_data: region_handshake_reply::AgentData { agent_id: agent_id(), session_id: session_id() },
        region_info: region_handshake_reply::RegionInfo { flags: 0 },
    });
    let packet = Packet::new(0x0102_0304, message.clone()).with_reliable();

    assert_eq!(packet.encode().expect("encode packet"), expected);
    assert_eq!(expected[0] & 0x80, 0x80);

    let decoded = Packet::decode(&expected).expect("decode fixture");
    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn movement_round_trip() {
    let expected = fixture(vectors::COMPLETE_AGENT_MOVEMENT);
    let message = Message::CompleteAgentMovement(CompleteAgentMovement {
        agent_data: complete_agent_movement::AgentData { agent_id: agent_id(), session_id: session_id(), circuit_code: 0x0102_0304 },
    });
    let packet = Packet::new(0x0102_0304, message.clone()).with_reliable();

    assert_eq!(packet.encode().expect("encode packet"), expected);
    let decoded = Packet::decode(&expected).expect("decode fixture");
    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn logout_request_round_trip() {
    let expected = fixture(vectors::LOGOUT_REQUEST);
    let message =
        Message::LogoutRequest(LogoutRequest { agent_data: logout_request::AgentData { agent_id: agent_id(), session_id: session_id() } });
    let packet = Packet::new(0x0102_0304, message.clone()).with_reliable();

    assert_eq!(packet.encode().expect("encode packet"), expected);
    let decoded = Packet::decode(&expected).expect("decode fixture");
    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn viewer_chat_round_trip() {
    let expected = fixture(vectors::CHAT_FROM_VIEWER);
    assert_eq!(SchemaMessage::schema_by_name("ChatFromViewer").expect("ChatFromViewer schema").encoding, MessageEncoding::Zerocoded);
    assert_eq!(expected[0] & 0x80, 0, "ChatFromViewer is sent unencoded despite its template flag");
    let message = chat_message("abc".into(), ChatType::Shout, 42);
    let packet = Packet::new(0x0102_0304, message.clone()).with_reliable();

    assert_eq!(packet.encode().expect("encode packet"), expected);
    let decoded = Packet::decode(&expected).expect("decode fixture");
    assert!(decoded.is_reliable());
    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn schema_chat_uses_unencoded_override() {
    let schema = SchemaMessage::schema_by_name("ChatFromViewer").expect("ChatFromViewer schema");
    let message = SchemaMessage::new(
        schema,
        vec![
            BlockValues::single(vec![FieldValue::UUID(agent_id()), FieldValue::UUID(session_id())]),
            BlockValues::single(vec![FieldValue::Bytes(vec![0; 4]), FieldValue::U8(1), FieldValue::S32(0)]),
        ],
    )
    .expect("valid schema message");
    let packet = Packet::new(1, Message::Schema(message));

    assert_eq!(packet.is_zerocoded(), Ok(false));
    assert_eq!(packet.encode().expect("encode schema chat")[0] & PacketFlags::ZEROCODED.bits(), 0);
}

#[test]
fn map_request_fixture() {
    let expected = fixture(vectors::MAP_BLOCK_REQUEST);
    let packet = Packet::new(
        0x0102_0304,
        MapBlockRequest {
            agent_data: map_block_request::AgentData {
                agent_id: agent_id(),
                session_id: session_id(),
                flags: 0,
                estate_id: 0,
                godlike: false,
            },
            position_data: PositionData { min_x: 32, max_x: 32, min_y: 44, max_y: 44 },
        }
        .into(),
    );

    assert_eq!(packet.encode().expect("encode map request"), expected);
}

#[test]
fn map_block_reply_fixture() {
    let bytes = fixture(vectors::MAP_BLOCK_REPLY);
    let packet = Packet::decode(&bytes).expect("decode map reply fixture");
    assert_eq!(packet.sequence(), 0x0102_0304);
    assert!(packet.is_reliable());
    assert!(matches!(
        packet.message(),
        Message::MapBlockReply(message)
            if message.agent_data.agent_id == agent_id()
                && message.agent_data.flags == 0
                && message.data.len() == 1
                && message.data[0].x == 32
                && message.data[0].y == 44
                && message.data[0].name == "Test"
                && message.data[0].access == 13
                && message.data[0].water_height == 20
                && message.data[0].map_image_id.is_nil()
    ));
}

#[test]
fn empty_ack_omits_count() {
    let empty_ack_bytes = fixture("00 01 02 03 04 00 ff ff ff fb");
    assert_eq!(Packet::decode(&empty_ack_bytes).expect("decode empty PacketAck").message(), &packet_ack(&[]));

    let map_block_reply =
        fixture(concat!("00 01 02 03 04 00 ff ff 01 99 ", "ff ee dd cc bb aa 99 88 77 66 55 44 33 22 11 00 ", "00 00 00 00"));
    assert_eq!(
        Packet::decode(&map_block_reply).expect("decode empty MapBlockReply").message(),
        &Message::MapBlockReply(MapBlockReply { agent_data: map_block_reply::AgentData { agent_id: agent_id(), flags: 0 }, data: vec![] })
    );
}

#[test]
fn logout_reply_empty_forms() {
    for bytes in [fixture(vectors::LOGOUT_REPLY_EMPTY), fixture(vectors::LOGOUT_REPLY_ZERO_COUNT)] {
        let decoded = Packet::decode(&bytes).expect("decode empty LogoutReply");
        assert_eq!(
            decoded.message(),
            &Message::LogoutReply(LogoutReply {
                agent_data: logout_reply::AgentData { agent_id: agent_id(), session_id: session_id() },
                inventory_data: vec![],
            })
        );
        assert!(decoded.trailing_message_bytes().is_empty());
    }
}

#[test]
fn logout_reply_nil_item() {
    let expected = fixture(vectors::LOGOUT_REPLY_NIL_ITEM);
    let decoded = Packet::decode(&expected).expect("decode nil-item LogoutReply");
    assert_eq!(
        decoded.message(),
        &Message::LogoutReply(LogoutReply {
            agent_data: logout_reply::AgentData { agent_id: agent_id(), session_id: session_id() },
            inventory_data: vec![InventoryData { item_id: UUID::nil() }],
        })
    );
    assert!(decoded.trailing_message_bytes().is_empty());
    assert_eq!(decoded.encode().expect("re-encode nil-item fixture"), expected);
}

#[test]
fn truncated_entries_rejected() {
    for bytes in [
        fixture("00 01 02 03 04 00 ff ff ff fb 01"),
        fixture(concat!(
            "00 01 02 03 04 00 ff ff 00 fd ",
            "ff ee dd cc bb aa 99 88 77 66 55 44 33 22 11 00 ",
            "00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff 01"
        )),
        fixture(concat!("00 01 02 03 04 00 ff ff 01 99 ", "ff ee dd cc bb aa 99 88 77 66 55 44 33 22 11 00 ", "00 00 00 00 01")),
    ] {
        assert_eq!(Packet::decode(&bytes), Err(LLUdpError::TruncatedPacket));
    }
}

#[test]
fn region_handle_fixtures() {
    let region_id = UUID::parse_str("12345678-1234-5678-1234-567812345678").expect("valid fixture region UUID");
    let request = Packet::new(0x0102_0304, RegionHandleRequest { request_block: RequestBlock { region_id } }.into());
    assert_eq!(request.encode().expect("encode region handle request"), fixture(vectors::REGION_HANDLE_REQUEST));

    let reply_bytes = fixture(vectors::REGION_ID_AND_HANDLE_REPLY);
    let decoded = Packet::decode(&reply_bytes).expect("decode region handle reply");
    assert!(matches!(
        decoded.message(),
        Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
            reply_block: ReplyBlock { region_id: id, region_handle: 0x0000_2000_0000_2c00 }
        }) if *id == region_id
    ));
    assert_eq!(decoded.encode().expect("re-encode region handle reply"), reply_bytes);
}

#[test]
fn packet_ack_round_trip() {
    let expected = fixture(vectors::PACKET_ACK);
    let message = packet_ack(&[0x1122_3344, 0xa1b2_c3d4]);
    let packet = Packet::new(0x0102_0304, message.clone());

    assert_eq!(packet.encode().expect("encode packet"), expected);
    let decoded = Packet::decode(&expected).expect("decode fixture");
    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn packet_ack_limit() {
    let packets = (0..=u8::MAX).map(|id| Packets { id: u32::from(id) }).collect();
    let message = Message::PacketAck(PacketAck { packets });

    assert_eq!(Packet::new(1, message).encode(), Err(LLUdpError::TooManyAcks));
}

#[test]
fn header_rejects_unrepresentable_fields() {
    let too_many_acks = Packet::new(1, complete_ping(1)).with_appended_acks(vec![0; usize::from(u8::MAX) + 1]);
    assert_eq!(too_many_acks.header(), Err(LLUdpError::TooManyAcks));
    assert_eq!(too_many_acks.is_zerocoded(), Err(LLUdpError::TooManyAcks));

    let long_extra_header = Packet::new(1, complete_ping(1)).with_extra_header(vec![0; usize::from(u8::MAX) + 1]);
    assert_eq!(long_extra_header.header(), Err(LLUdpError::ExtraHeaderTooLong));
}

#[test]
fn encode_into_appends() {
    let expected = fixture(vectors::START_PING_CHECK);
    let packet = Packet::decode(&expected).expect("decode fixture");
    let mut output = b"prefix".to_vec();

    packet.encode_into(&mut output).expect("append packet bytes");

    assert_eq!(&output[..6], b"prefix");
    assert_eq!(&output[6..], expected);
}

#[test]
fn encode_into_preserves_output_on_error() {
    let packet = Packet::new(1, chat_message("x".repeat(usize::from(u16::MAX) + 1), ChatType::Normal, 0));
    let mut output = b"prefix".to_vec();

    assert_eq!(packet.encode_into(&mut output), Err(LLUdpError::VariableFieldTooLong));
    assert_eq!(output, b"prefix");
}

#[test]
fn packet_header_is_typed() {
    let source = Packet::new(0x0102_0304, complete_ping(7)).with_reliable().with_extra_header(vec![0xaa, 0xbb]).with_appended_acks(vec![3]);
    let bytes = source.encode().expect("encode header fixture");
    let decoded = Packet::decode(&bytes).expect("decode header fixture");
    let header = decoded.header().expect("read packet header");

    assert_eq!(header.sequence(), 0x0102_0304);
    assert!(header.flags().contains(PacketFlags::RELIABLE));
    assert!(header.flags().contains(PacketFlags::APPENDED_ACKS));
    assert_eq!(header.extra_header(), &[0xaa, 0xbb]);
    assert_eq!(decoded.appended_acks(), &[3]);
}

#[test]
fn header_bits_survive_round_trip() {
    let mut bytes = Packet::new(1, complete_ping(9)).with_zero_coding(false).encode().expect("encode packet");
    bytes[0] |= 0x01;
    let decoded = Packet::decode(&bytes).expect("decode packet with unknown flag");

    assert!(decoded.header().expect("read packet header").flags().contains(PacketFlags::from_bits_retain(0x01)));
    assert_eq!(decoded.encode().expect("re-encode packet flags"), bytes);
}

#[test]
fn header_flags_match_wire() {
    assert_eq!(
        SchemaMessage::schema_by_name("RegionHandshakeReply").expect("RegionHandshakeReply schema").encoding,
        MessageEncoding::Zerocoded
    );
    let id = UUID::from_bytes([0x11; 16]);
    let packet = Packet::new(
        1,
        Message::RegionHandshakeReply(RegionHandshakeReply {
            agent_data: region_handshake_reply::AgentData { agent_id: id, session_id: id },
            region_info: region_handshake_reply::RegionInfo { flags: u32::MAX },
        }),
    );

    let header = packet.header().expect("read effective packet header");
    let bytes = packet.encode().expect("encode packet");

    assert_eq!(header.flags().bits(), bytes[0]);
    assert_eq!(bytes[0], 0, "zero coding must be omitted when it expands the message");
    assert!(!header.flags().contains(PacketFlags::ZEROCODED));
    assert_eq!(packet.is_zerocoded(), Ok(false));
    assert_eq!(Packet::decode(&bytes).expect("decode packet").header().expect("read decoded header").flags(), header.flags());
}

#[test]
fn empty_appended_ack_section_survives() {
    let mut bytes = Packet::new(1, complete_ping(9)).with_zero_coding(false).encode().expect("encode packet");
    bytes[0] |= PacketFlags::APPENDED_ACKS.bits();
    bytes.push(0);
    let decoded = Packet::decode(&bytes).expect("decode empty appended ACK section");

    assert!(decoded.appended_acks().is_empty());
    assert_eq!(decoded.encode().expect("re-encode empty ACK section"), bytes);
}

#[test]
fn resent_requires_reliable() {
    let bytes = [0x20, 0x01, 0x02, 0x03, 0x04, 0x00];

    assert_eq!(Packet::decode(&bytes), Err(LLUdpError::InvalidPacketFlags));
}

#[test]
fn ack_trailer_round_trip() {
    let expected = fixture(vectors::APPENDED_ACKS);
    let decoded = Packet::decode(&expected).expect("decode fixture");

    assert_eq!(decoded.sequence(), 0x0102_0304);
    assert!(decoded.is_reliable());
    assert!(!decoded.is_resent());
    assert_eq!(decoded.message(), &complete_ping(7));
    assert_eq!(decoded.appended_acks(), &[0x1122_3344, 0xa1b2_c3d4]);
    assert_eq!(decoded.encode().expect("re-encode fixture"), expected);
}

#[test]
fn start_ping_check_fixture() {
    let expected = fixture(vectors::START_PING_CHECK);
    let message = start_ping(7, 0x0506_0708);
    let packet = Packet::new(0x0102_0304, message.clone());

    assert_eq!(packet.encode().expect("encode ping check"), expected);
    let decoded = Packet::decode(&expected).expect("decode ping check fixture");
    assert_eq!(decoded.message(), &message);
}

#[test]
fn unknown_high_id_round_trip() {
    let expected = fixture("00 00 00 00 07 00 7e de ad be ef");
    let message = Message::Unknown { id: MessageId::high(0x7e), payload: vec![0xde, 0xad, 0xbe, 0xef] };
    let decoded = Packet::decode(&expected).expect("decode unknown message");

    assert_eq!(decoded.message(), &message);
    assert_eq!(decoded.encode().expect("re-encode unknown message"), expected);
}

#[test]
fn typed_ext_round_trip() {
    let packet = Packet::new(7, complete_ping(3)).with_zero_coding(false);
    let mut expected = packet.encode().expect("encode typed packet");
    expected.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

    let decoded = Packet::decode(&expected).expect("decode typed packet with extension");
    assert_eq!(decoded.message(), &complete_ping(3));
    assert_eq!(decoded.encode().expect("re-encode typed packet with extension"), expected);
}

#[test]
fn schema_ext_round_trip() {
    let mut expected = fixture("00 00 00 00 07 00 ff ff ff fd");
    expected.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

    let decoded = Packet::decode(&expected).expect("decode generic template packet");
    assert!(matches!(
        decoded.message(),
        Message::Schema(message)
            if message.schema().name == "CloseCircuit"
                && message.trailing_bytes() == [0xde, 0xad, 0xbe, 0xef]
    ));
    assert_eq!(decoded.encode().expect("re-encode generic template packet"), expected);
}

#[test]
fn schema_zerocoding() {
    let schema = SchemaMessage::schema_by_name("ViewerEffect").expect("ViewerEffect compiled schema");
    let message = SchemaMessage::new(
        schema,
        vec![
            BlockValues::single(vec![FieldValue::UUID(UUID::nil()), FieldValue::UUID(UUID::nil())]),
            BlockValues::new(vec![BlockInstance::new(vec![
                FieldValue::UUID(UUID::nil()),
                FieldValue::UUID(UUID::nil()),
                FieldValue::U8(0),
                FieldValue::F32(0.0),
                FieldValue::Bytes(vec![0; 4]),
                FieldValue::Bytes(Vec::new()),
            ])]),
        ],
    )
    .expect("valid ViewerEffect");
    let packet = Packet::new(1, Message::Schema(message));
    let header = packet.header().expect("read effective packet header");
    let encoded = packet.encode().expect("encode ViewerEffect");
    assert_eq!(header.flags().bits(), encoded[0]);
    assert!(header.flags().contains(PacketFlags::ZEROCODED), "template metadata enables zero coding when it saves bytes");
}

#[test]
fn trust_keeps_unreliable() {
    let schema = SchemaMessage::schema_by_name("ObjectUpdateCompressed").expect("trusted compiled schema");
    let message = SchemaMessage::new(
        schema,
        vec![
            BlockValues::single(vec![FieldValue::U64(0), FieldValue::U16(0)]),
            BlockValues::new(vec![BlockInstance::new(vec![FieldValue::U32(0), FieldValue::Bytes(vec![0xaa, 0xbb])])]),
        ],
    )
    .expect("valid ObjectUpdateCompressed");
    let message = Message::Schema(message);
    assert!(message.requires_trusted_circuit());
    assert!(!Packet::new(1, message).is_reliable());
}

#[test]
fn malformed_packets_rejected() {
    assert!(matches!(Packet::decode(&[]), Err(LLUdpError::TruncatedPacket)));
    assert!(matches!(Packet::decode(&[0, 0, 0, 0, 1]), Err(LLUdpError::TruncatedPacket)));
    assert!(matches!(Packet::decode(&[0, 0, 0, 0, 1, 1]), Err(LLUdpError::TruncatedPacket)));
    assert!(matches!(Packet::decode(&[0, 0, 0, 0, 1, 0, 0xff, 0xff, 0x00]), Err(LLUdpError::TruncatedPacket)));
    assert!(matches!(Packet::decode(&[0, 0, 0, 0, 1, 0, 0xff, 0xff, 0x00, 0x00]), Err(LLUdpError::InvalidMessageId)));
    assert!(matches!(Packet::decode(&[0x80, 0, 0, 0, 1, 0, 0x04, 0]), Err(LLUdpError::InvalidZeroCode)));
    assert!(matches!(Packet::decode(&[0x10, 0, 0, 0, 1, 0, 0x04, 1, 2, 3, 4, 2]), Err(LLUdpError::InvalidAcknowledgementList)));
}

#[test]
fn message_id_boundaries() {
    let invalid_ids: &[&[u8]] = &[&[0x00], &[0xff, 0x00], &[0xff, 0xff, 0x00, 0x00]];
    for &id in invalid_ids {
        let mut packet = vec![0, 0, 0, 0, 1, 0];
        packet.extend_from_slice(id);
        assert_eq!(Packet::decode(&packet).expect_err("reserved ID must fail"), LLUdpError::InvalidMessageId);
    }

    let mut low_packet = vec![0, 0, 0, 0, 1, 0];
    low_packet.extend_from_slice(&[0xff, 0xff, 0xff, 0xf9]);
    let decoded = Packet::decode(&low_packet).expect("highest low ID is valid");
    assert_eq!(decoded.message(), &Message::Unknown { id: MessageId::low(0xfff9), payload: Vec::new() });
}

#[test]
fn rejects_oversize_inputs() {
    let ping = complete_ping(1);

    assert_eq!(Packet::new(1, ping.clone()).with_extra_header(vec![0; 256]).encode(), Err(LLUdpError::ExtraHeaderTooLong));
    assert_eq!(Packet::new(1, ping).with_appended_acks(vec![0; 256]).encode(), Err(LLUdpError::TooManyAcks));

    let oversized_chat = chat_message("x".repeat(65_535), ChatType::Normal, 0);
    assert_eq!(Packet::new(1, oversized_chat).encode(), Err(LLUdpError::VariableFieldTooLong));
}
