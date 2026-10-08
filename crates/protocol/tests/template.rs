#[path = "../build/template.rs"]
mod parser;

use protocol::udp::MessageId;
use protocol::{
    Frequency,
    schema::{
        BlockMode, FieldKind, MESSAGE_COUNT, MESSAGES,
        ids::{
            AGENT_MOVEMENT_COMPLETE, AGENT_UPDATE, CHAT_FROM_SIMULATOR, CHAT_FROM_VIEWER, COMPLETE_AGENT_MOVEMENT, COMPLETE_PING_CHECK,
            LOGOUT_REPLY, LOGOUT_REQUEST, MAP_BLOCK_REPLY, MAP_BLOCK_REQUEST, PACKET_ACK, REGION_HANDLE_REQUEST, REGION_HANDSHAKE,
            REGION_HANDSHAKE_REPLY, REGION_ID_AND_HANDLE_REPLY, START_PING_CHECK, USE_CIRCUIT_CODE,
        },
    },
};

#[test]
fn schema_covers_messages() {
    assert_eq!(MESSAGE_COUNT, 483);
    assert_eq!(MESSAGES.len(), MESSAGE_COUNT);

    let names = MESSAGES.iter().map(|message| message.name).collect::<Vec<_>>();
    let unique = names.iter().copied().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), names.len());
}

#[test]
fn ids_cover_typed_messages() {
    let ids = [
        ("UseCircuitCode", USE_CIRCUIT_CODE),
        ("ChatFromViewer", CHAT_FROM_VIEWER),
        ("ChatFromSimulator", CHAT_FROM_SIMULATOR),
        ("RegionHandleRequest", REGION_HANDLE_REQUEST),
        ("RegionIDAndHandleReply", REGION_ID_AND_HANDLE_REPLY),
        ("RegionHandshake", REGION_HANDSHAKE),
        ("RegionHandshakeReply", REGION_HANDSHAKE_REPLY),
        ("CompleteAgentMovement", COMPLETE_AGENT_MOVEMENT),
        ("AgentMovementComplete", AGENT_MOVEMENT_COMPLETE),
        ("MapBlockRequest", MAP_BLOCK_REQUEST),
        ("MapBlockReply", MAP_BLOCK_REPLY),
        ("AgentUpdate", AGENT_UPDATE),
        ("StartPingCheck", START_PING_CHECK),
        ("CompletePingCheck", COMPLETE_PING_CHECK),
        ("PacketAck", PACKET_ACK),
        ("LogoutRequest", LOGOUT_REQUEST),
        ("LogoutReply", LOGOUT_REPLY),
    ];

    for (name, id) in ids {
        let schema = MESSAGES.iter().find(|message| message.name == name).unwrap_or_else(|| panic!("{name} is in the vendored template"));
        assert_eq!(schema.id, id, "generated constant for {name}");
    }
}

#[test]
fn schema_block_layouts() {
    let message = MESSAGES.iter().find(|message| message.name == "ChatFromViewer").expect("ChatFromViewer is in the template");
    assert_eq!(message.id.frequency, Frequency::Low);
    assert_eq!(message.id.number, 80);
    assert_eq!(message.blocks.len(), 2);
    let chat_data = message.blocks.iter().find(|block| block.name == "ChatData").expect("ChatData block is in the template");
    assert_eq!(chat_data.mode, BlockMode::Single);
    assert!(chat_data.fields.iter().any(|field| field.name == "Message" && field.kind == FieldKind::Variable(2)));

    let message = MESSAGES.iter().find(|message| message.name == "MapBlockReply").expect("MapBlockReply is in the template");
    assert!(message.blocks.iter().any(|block| block.name == "Data" && block.mode == BlockMode::Variable));
}

#[test]
fn lookup_covers_messages() {
    for schema in MESSAGES {
        assert_eq!(protocol::SchemaMessage::schema_by_id(schema.id), Some(schema));
        assert_eq!(protocol::SchemaMessage::schema_by_name(schema.name), Some(schema));
    }
    assert_eq!(protocol::SchemaMessage::schema_by_name("missing"), None);
}

#[test]
fn generator_emits_schema() {
    let source = include_str!("../build/message_template.msg");
    let parsed = parser::parse("build/message_template.msg", source).expect("official template parses");
    let generated = parser::generate(&parsed);

    assert!(generated.contains("MessageSchema"));
    assert!(generated.contains("FieldKind::Variable(2)"));
    assert!(generated.contains("const MESSAGE_ID_INDEX: &[usize] = &[\n"));
    assert!(generated.contains("binary_search_by_key"));
    assert!(generated.contains("FieldSchema {\n                        name:"));
    assert!(generated.contains("pub const USE_CIRCUIT_CODE: MessageId = MessageId::low(3)"));
}

#[test]
fn ids_keep_frequency_bits() {
    assert_eq!(USE_CIRCUIT_CODE, MessageId::low(3));
    assert_eq!(AGENT_UPDATE, MessageId::high(4));
    assert_eq!(PACKET_ACK, MessageId::fixed(0xffff_fffb));
}
