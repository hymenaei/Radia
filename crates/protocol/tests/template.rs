#[path = "../build/generate.rs"]
mod generate;
#[path = "../build/template.rs"]
mod template;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use protocol::lludp::{AllocationStrategy, Message, MessageId, Packet};
use protocol::{
    BlockValues, FieldValue, Frequency, LLUdpError, SchemaMessage,
    schema::{
        BlockMode, FieldKind, MESSAGE_COUNT, MESSAGES, MessageFlag,
        ids::{
            AGENT_MOVEMENT_COMPLETE, AGENT_THROTTLE, AGENT_UPDATE, CHAT_FROM_SIMULATOR, CHAT_FROM_VIEWER, CLOSE_CIRCUIT,
            COMPLETE_AGENT_MOVEMENT, COMPLETE_PING_CHECK, CONFIRM_ENABLE_SIMULATOR, DISABLE_SIMULATOR, ENABLE_SIMULATOR,
            GENERIC_STREAMING_MESSAGE, IMPROVED_TERSE_OBJECT_UPDATE, KILL_OBJECT, LOGOUT_REPLY, LOGOUT_REQUEST, MAP_BLOCK_REPLY,
            MAP_BLOCK_REQUEST, OBJECT_EXTRA_PARAMS, OBJECT_UPDATE, OBJECT_UPDATE_CACHED, OBJECT_UPDATE_COMPRESSED, OPEN_CIRCUIT,
            PACKET_ACK, REGION_HANDLE_REQUEST, REGION_HANDSHAKE, REGION_HANDSHAKE_REPLY, REGION_ID_AND_HANDLE_REPLY,
            REQUEST_MULTIPLE_OBJECTS, START_PING_CHECK, TELEPORT_CANCEL, USE_CIRCUIT_CODE,
        },
    },
};
use testing::{lludp::vectors, parse_hex_fixture};

struct BlockFacts {
    name: &'static str,
    mode: BlockMode,
    fields: &'static str,
}

struct TypedMessageCoverage {
    name: &'static str,
    id: MessageId,
    fixture: &'static str,
    preflight: AllocationStrategy,
    conformance: ConformanceCategory,
    blocks: &'static [BlockFacts],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConformanceCategory {
    SchemaConforming,
    SemanticOverride,
    CompatibilityOverride,
    PackedOverride,
}

macro_rules! typed_message_coverage {
    ($(
        $name:literal => $variant:ident, $id:ident, $fixture:expr,
        $preflight:ident, $conformance:ident, [
            $(($block:literal, $mode:expr, $fields:literal)),* $(,)?
        ];
    )+) => {
        const TYPED_MESSAGE_COVERAGE: &[TypedMessageCoverage] = &[
            $(TypedMessageCoverage {
                name: $name,
                id: $id,
                fixture: $fixture,
                preflight: AllocationStrategy::$preflight,
                conformance: ConformanceCategory::$conformance,
                blocks: &[$(BlockFacts { name: $block, mode: $mode, fields: $fields }),*],
            }),+
        ];

        fn typed_variant_name(message: &Message) -> Option<&'static str> {
            match message {
                $(Message::$variant(_) => Some($name),)+
                Message::Schema(_) | Message::Unknown { .. } => None,
            }
        }
    };
}

include!("support/typed_coverage.rs");

#[test]
fn schema_covers_messages() {
    assert_eq!(MESSAGE_COUNT, 483);
    assert_eq!(MESSAGES.len(), MESSAGE_COUNT);

    let names = MESSAGES.iter().map(|message| message.name).collect::<Vec<_>>();
    let unique = names.iter().copied().collect::<HashSet<_>>();
    assert_eq!(unique.len(), names.len());
}

#[test]
fn typed_inventory_matches_schema_and_fixtures() {
    assert_eq!(TYPED_MESSAGE_COVERAGE.len(), 30);
    let mut schema_conforming = 0;
    let mut semantic_overrides = 0;
    let mut compatibility_overrides = 0;
    let mut packed_overrides = 0;
    let mut ids = Vec::new();

    for coverage in TYPED_MESSAGE_COVERAGE {
        assert!(!ids.contains(&coverage.id), "typed message ID {:?} is listed once", coverage.id);
        ids.push(coverage.id);
        let schema = SchemaMessage::schema_by_id(coverage.id).unwrap_or_else(|| panic!("{} has generated schema metadata", coverage.name));
        assert_eq!(schema.name, coverage.name, "generated name for {}", coverage.name);
        assert_eq!(schema.id, coverage.id, "generated ID for {}", coverage.name);
        assert_eq!(schema.blocks.len(), coverage.blocks.len(), "block count for {}", coverage.name);

        match coverage.conformance {
            ConformanceCategory::SchemaConforming => schema_conforming += 1,
            ConformanceCategory::SemanticOverride => semantic_overrides += 1,
            ConformanceCategory::CompatibilityOverride => compatibility_overrides += 1,
            ConformanceCategory::PackedOverride => packed_overrides += 1,
        }

        for (block, expected) in schema.blocks.iter().zip(coverage.blocks) {
            assert_eq!(block.name, expected.name, "block order for {}", coverage.name);
            assert_eq!(block.mode, expected.mode, "block mode for {}.{}", coverage.name, expected.name);
            let actual_fields = block.fields.iter().map(|field| field.name).collect::<Vec<_>>();
            let expected_fields = expected.fields.split_ascii_whitespace().collect::<Vec<_>>();
            assert_eq!(actual_fields, expected_fields, "field order and count for {}.{}", coverage.name, expected.name);
        }

        let bytes = parse_hex_fixture(coverage.fixture).expect("typed fixture is valid hex");
        let preflight = Packet::preflight(&bytes).unwrap_or_else(|error| panic!("preflight {} fixture: {error}", coverage.name));
        assert_eq!(preflight.message_id(), coverage.id, "preflight ID for {}", coverage.name);
        assert_eq!(Message::typed_allocation_strategy(coverage.id), Some(coverage.preflight), "typed route for {}", coverage.name);
        assert_eq!(Message::allocation_strategy(coverage.id), coverage.preflight, "registered route for {}", coverage.name);
        assert_eq!(preflight.allocation_strategy(), coverage.preflight, "preflight route for {}", coverage.name);

        let packet = Packet::decode(&bytes).unwrap_or_else(|error| panic!("decode {} fixture: {error}", coverage.name));
        assert_eq!(typed_variant_name(packet.message()), Some(coverage.name), "typed dispatch for {}", coverage.name);
        let reencoded = packet.encode().expect("re-encode typed fixture");
        let encoded_preflight = Packet::preflight(&reencoded).expect("preflight re-encoded typed fixture");
        assert_eq!(encoded_preflight.message_id(), coverage.id, "encoded ID for {}", coverage.name);
        assert_eq!(encoded_preflight.allocation_strategy(), coverage.preflight, "encoded route for {}", coverage.name);
        if coverage.name == "ImprovedTerseObjectUpdate" {
            let canonical = Packet::decode(&reencoded).expect("decode canonical terse fixture");
            assert_eq!(canonical.message(), packet.message(), "semantic values for {}", coverage.name);
        } else {
            assert_eq!(reencoded, bytes, "wire bytes for {}", coverage.name);
        }
    }

    assert_eq!(schema_conforming + semantic_overrides + compatibility_overrides + packed_overrides, TYPED_MESSAGE_COVERAGE.len());
    assert!(schema_conforming > 0);
    assert!(semantic_overrides > 0);
    assert!(compatibility_overrides > 0);
    assert!(packed_overrides > 0);
}

#[test]
fn compatibility_and_packed_overrides_remain_explicit() {
    let category = |name: &str| TYPED_MESSAGE_COVERAGE.iter().find(|coverage| coverage.name == name).map(|coverage| coverage.conformance);
    assert_eq!(category("RegionHandshake"), Some(ConformanceCategory::CompatibilityOverride));
    assert_eq!(category("LogoutReply"), Some(ConformanceCategory::CompatibilityOverride));
    assert_eq!(category("ObjectUpdate"), Some(ConformanceCategory::PackedOverride));
    assert_eq!(category("ObjectUpdateCompressed"), Some(ConformanceCategory::PackedOverride));
    assert_eq!(category("ImprovedTerseObjectUpdate"), Some(ConformanceCategory::PackedOverride));

    let omitted = parse_hex_fixture(vectors::LOGOUT_REPLY_EMPTY).expect("omitted-count fixture is valid hex");
    let explicit_zero = parse_hex_fixture(vectors::LOGOUT_REPLY_ZERO_COUNT).expect("zero-count fixture is valid hex");
    let decoded_omitted = Packet::decode(&omitted).expect("decode omitted final variable-block count");
    assert_eq!(typed_variant_name(decoded_omitted.message()), Some("LogoutReply"));
    let canonical = decoded_omitted.encode().expect("encode compatibility-generated zero count");
    assert_eq!(canonical, explicit_zero, "an omitted count becomes a wire zero count, not a synthetic inventory item");
    assert_ne!(canonical, omitted, "the encoder makes compatibility padding visible on wire");
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
fn lludp_flags_match_template_tokens() {
    let blacklisted = MESSAGES.iter().find(|message| message.name == "OpenCircuit").expect("OpenCircuit is in the template");
    assert_eq!(blacklisted.flags, &[MessageFlag::LLUdpBlacklisted]);

    let deprecated =
        MESSAGES.iter().find(|message| message.name == "SetSimPresenceInDatabase").expect("SetSimPresenceInDatabase is in the template");
    assert_eq!(deprecated.flags, &[MessageFlag::LLUdpDeprecated]);
}

#[test]
fn lookup_covers_messages() {
    for schema in MESSAGES {
        assert_eq!(SchemaMessage::schema_by_id(schema.id), Some(schema));
        assert_eq!(SchemaMessage::schema_by_name(schema.name), Some(schema));
    }
    assert_eq!(SchemaMessage::schema_by_name("missing"), None);
}

#[test]
fn generator_emits_schema() {
    let source = include_str!("../build/message_template.msg");
    let parsed = template::parse("build/message_template.msg", source).expect("pinned official template parses");
    let generated = generate::generate(&parsed);
    let generated_again = generate::generate(&parsed);

    assert_eq!(generated, generated_again, "generation is deterministic");
    assert!(generated.contains("MessageSchema"));
    assert!(generated.contains("FieldKind::Variable(2)"));
    assert!(generated.contains("const MESSAGE_ID_INDEX: &[usize] = &[\n"));
    assert!(generated.contains("binary_search_by_key"));
    assert!(generated.contains("FieldSchema {\n                        name:"));
    assert!(generated.contains("pub const USE_CIRCUIT_CODE: MessageId = MessageId::low(3)"));
}

#[test]
fn fixed_message_ids_cover_the_fixed_frequency_range() {
    let endpoints = [("PacketAck", PACKET_ACK), ("OpenCircuit", OPEN_CIRCUIT), ("CloseCircuit", CLOSE_CIRCUIT)];
    for (name, id) in endpoints {
        assert_eq!(id.frequency, Frequency::Fixed);
        assert_eq!(SchemaMessage::schema_by_id(id).map(|schema| schema.name), Some(name));
    }
}

#[test]
fn schema_created_typed_ids_still_use_strict_typed_decode() {
    let schema = SchemaMessage::schema_by_id(COMPLETE_PING_CHECK).expect("CompletePingCheck schema");
    let message = SchemaMessage::new(schema, vec![BlockValues::single(vec![FieldValue::U8(0x5a)])]).expect("valid schema message");
    let fixture = parse_hex_fixture(vectors::COMPLETE_PING_CHECK).expect("valid fixture hex");
    let encoded = Packet::new(0x0102_0304, Message::Schema(message)).encode().expect("encode schema message");
    assert_eq!(encoded, fixture);

    let decoded = Packet::decode(&fixture).expect("decode typed message ID");
    assert_eq!(typed_variant_name(decoded.message()), Some("CompletePingCheck"));

    let mut truncated = fixture;
    truncated.pop();
    assert_eq!(Packet::decode(&truncated).expect_err("typed payload cannot zero-fill its required field"), LLUdpError::TruncatedPacket);
}

#[test]
fn typed_preflight_covers_suffixes_and_zero_coding() {
    for (fixture, expected_name) in [
        (vectors::REGION_HANDSHAKE_LEGACY, "RegionHandshake"),
        (vectors::REGION_HANDSHAKE_INFO4, "RegionHandshake"),
        (vectors::CHAT_FROM_VIEWER_ZEROCODED, "ChatFromViewer"),
        (vectors::AGENT_UPDATE_ZEROCODED, "AgentUpdate"),
    ] {
        let bytes = parse_hex_fixture(fixture).expect("edge fixture is valid hex");
        let packet = Packet::decode(&bytes).expect("decode typed edge fixture");
        assert_eq!(typed_variant_name(packet.message()), Some(expected_name));
        assert_eq!(packet.encode().expect("re-encode typed edge fixture"), bytes);
    }
}

#[test]
fn ids_keep_frequency_bits() {
    assert_eq!(USE_CIRCUIT_CODE, MessageId::low(3));
    assert_eq!(AGENT_UPDATE, MessageId::high(4));
    assert_eq!(PACKET_ACK, MessageId::fixed(0xffff_fffb));
}

fn canonical_layout(template: &template::Template) -> String {
    let mut messages = template.messages.iter().collect::<Vec<_>>();
    messages.sort_by_key(|message| (frequency_order(message.frequency), message.id));

    let mut layout = String::new();
    for message in messages {
        let frequency = match message.frequency {
            template::Frequency::High => "High",
            template::Frequency::Medium => "Medium",
            template::Frequency::Low => "Low",
            template::Frequency::Fixed => "Fixed",
        };
        write!(&mut layout, "{frequency}:{} {}", message.id, message.name).expect("write canonical message ID");

        for block in &message.blocks {
            let mode = match block.mode {
                template::BlockMode::Single => "Single".to_owned(),
                template::BlockMode::Variable => "Variable".to_owned(),
                template::BlockMode::Multiple(count) => format!("Multiple({count})"),
            };
            write!(&mut layout, " | {}:{mode}", block.name).expect("write canonical block");
            for field in &block.fields {
                write!(&mut layout, " {}:{}", field.name, canonical_wire_type(field.kind)).expect("write canonical field");
            }
        }
        layout.push('\n');
    }
    layout
}

fn frequency_order(frequency: template::Frequency) -> u8 {
    match frequency {
        template::Frequency::High => 0,
        template::Frequency::Medium => 1,
        template::Frequency::Low => 2,
        template::Frequency::Fixed => 3,
    }
}

fn canonical_wire_type(kind: template::FieldKind) -> String {
    match kind {
        template::FieldKind::Bool => "BOOL".to_owned(),
        template::FieldKind::F32 => "F32".to_owned(),
        template::FieldKind::F64 => "F64".to_owned(),
        template::FieldKind::IpAddr => "IPADDR".to_owned(),
        template::FieldKind::IpPort => "IPPORT".to_owned(),
        template::FieldKind::Null => "NULL".to_owned(),
        template::FieldKind::Quaternion => "LLQuaternion".to_owned(),
        template::FieldKind::Uuid => "LLUUID".to_owned(),
        template::FieldKind::Vector3 => "LLVector3".to_owned(),
        template::FieldKind::Vector3d => "LLVector3d".to_owned(),
        template::FieldKind::Vector4 => "LLVector4".to_owned(),
        template::FieldKind::S8 => "S8".to_owned(),
        template::FieldKind::S16 => "S16".to_owned(),
        template::FieldKind::S32 => "S32".to_owned(),
        template::FieldKind::S64 => "S64".to_owned(),
        template::FieldKind::U8 => "U8".to_owned(),
        template::FieldKind::U16 => "U16".to_owned(),
        template::FieldKind::U32 => "U32".to_owned(),
        template::FieldKind::U64 => "U64".to_owned(),
        template::FieldKind::Fixed(size) => format!("Fixed({size})"),
        template::FieldKind::Variable(prefix) => format!("Variable({prefix})"),
    }
}

#[test]
fn canonical_template_layout_matches_reviewed_snapshot() {
    let source = include_str!("../build/message_template.msg");
    let parsed = template::parse("build/message_template.msg", source).expect("pinned official template parses");
    let actual = canonical_layout(&parsed);
    let expected = normalize_newlines(include_str!("fixtures/message_layout.txt"));
    assert!(expected.ends_with('\n'), "reviewed layout snapshot ends with a newline");
    assert!(actual.ends_with('\n'), "canonical layout ends with a newline");
    assert_eq!(expected, actual, "reviewed message layout changed:\n{}", layout_diff(&expected, &actual));

    let normalized_source = normalize_newlines(source);
    let crlf_source = normalized_source.replace('\n', "\r\n");
    let crlf_template = template::parse("build/message_template.msg", &crlf_source).expect("CRLF template parses");
    assert_eq!(actual, canonical_layout(&crlf_template), "canonical layout is independent of line endings");
}

fn normalize_newlines(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

fn layout_diff(expected: &str, actual: &str) -> String {
    let by_message_id = |layout: &str| {
        layout
            .lines()
            .map(|line| {
                let id = line.split_ascii_whitespace().next().expect("canonical layout line has a message ID");
                (id.to_owned(), line.to_owned())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let expected = by_message_id(expected);
    let actual = by_message_id(actual);
    let message_ids = expected.keys().chain(actual.keys()).cloned().collect::<BTreeSet<_>>();

    let mut diff = String::new();
    for id in message_ids {
        match (expected.get(&id), actual.get(&id)) {
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some(expected), Some(actual)) => {
                writeln!(&mut diff, "- {expected}").expect("write expected layout diff line");
                writeln!(&mut diff, "+ {actual}").expect("write actual layout diff line");
            }
            (Some(expected), None) => writeln!(&mut diff, "- {expected}").expect("write removed layout diff line"),
            (None, Some(actual)) => writeln!(&mut diff, "+ {actual}").expect("write added layout diff line"),
            (None, None) => unreachable!("message ID came from one of the maps"),
        }
    }
    diff
}
