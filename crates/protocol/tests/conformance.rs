use math::{Quat, Vec3};
use proptest::{prelude::*, test_runner::Config};
use uuid::Uuid;

use protocol::{
    BlockInstance, BlockValues, ChatType, FieldValue, SchemaMessage,
    lludp::{Message, MessageId, Packet},
    messages::{
        AgentUpdate, ChatFromViewer, RegionHandshake, agent_update, chat_from_viewer,
        region_handshake::{RegionInfo1, RegionInfo2, RegionInfo3, RegionInfo4},
    },
    schema::{self, ids::*},
};
use testing::{lludp::vectors, parse_hex_fixture};

struct CoverageId {
    name: &'static str,
    id: MessageId,
    fixture: &'static str,
    conformance: &'static str,
}

macro_rules! typed_message_coverage {
    ($(
        $name:literal => $variant:ident, $id:ident, $fixture:expr,
        $preflight:ident, $conformance:ident, [
            $(($block:literal, $mode:expr, $fields:literal)),* $(,)?
        ];
    )+) => {
        const TYPED_MESSAGE_IDS: &[CoverageId] = &[
            $(CoverageId { name: $name, id: $id, fixture: $fixture, conformance: stringify!($conformance) }),+
        ];
    };
}

include!("support/typed_coverage.rs");

fn encode_packet(message: Message) -> Vec<u8> {
    Packet::new(0x0102_0304, message).encode().expect("encode protocol packet")
}

fn decode_packet(bytes: &[u8]) -> Message {
    Packet::decode(bytes).expect("decode protocol packet").into_message()
}

fn packet_message_id(bytes: &[u8]) -> MessageId {
    Packet::preflight(bytes).expect("preflight protocol packet").message_id()
}

#[test]
fn decoded_inventory_messages_keep_their_declared_ids() {
    for coverage in TYPED_MESSAGE_IDS {
        let bytes = parse_hex_fixture(coverage.fixture).expect("inventory fixture is valid hex");
        let _packet = Packet::decode(&bytes).unwrap_or_else(|error| panic!("decode {} fixture: {error}", coverage.name));
        assert_eq!(packet_message_id(&bytes), coverage.id, "decoded message ID for {}", coverage.name);
    }
}

#[test]
fn schema_conforming_messages_match_wire_layout() {
    for coverage in TYPED_MESSAGE_IDS.iter().filter(|coverage| coverage.conformance == "SchemaConforming") {
        let bytes = parse_hex_fixture(coverage.fixture).expect("inventory fixture is valid hex");
        let packet = Packet::decode(&bytes).unwrap_or_else(|error| panic!("decode {} fixture: {error}", coverage.name));
        let typed = packet.message().clone();
        let message_schema = schema::find_by_id(coverage.id).expect("inventory ID has generated schema");
        let expected_fields = schema_conforming::schema_conforming_fields(&typed)
            .unwrap_or_else(|| panic!("{} has a schema-conforming typed field mapping", coverage.name));
        let generic = SchemaMessage::new(message_schema, expected_fields.clone())
            .unwrap_or_else(|error| panic!("construct generic {} fixture: {error}", coverage.name));
        assert_eq!(generic.blocks(), expected_fields, "fixture field meaning and order for {}", coverage.name);

        let typed_wire = encode_packet(typed.clone());
        let generic_wire = encode_packet(Message::Schema(generic));
        assert_eq!(generic_wire, typed_wire, "schema field layout for {}", coverage.name);
        assert_eq!(packet_message_id(&generic_wire), coverage.id, "schema packet ID for {}", coverage.name);
        assert_eq!(decode_packet(&generic_wire), typed, "generic packet dispatches to typed {}", coverage.name);
    }
}

mod schema_conforming {
    include!("support/schema_conforming.rs");
}

mod schema_values {
    include!("support/schema_values.rs");
}

use schema_values::generated_schema_blocks;

proptest! {
    #![proptest_config(Config { cases: 32, ..Config::default() })]

    #[test]
    fn schema_values_match_typed_wire(seed in any::<u64>()) {
        for coverage in TYPED_MESSAGE_IDS.iter().filter(|coverage| coverage.conformance == "SchemaConforming") {
            let message_schema = schema::find_by_id(coverage.id).expect("inventory ID has generated schema");
            let expected_fields = generated_schema_blocks(message_schema, seed);
            let generic = SchemaMessage::new(message_schema, expected_fields.clone())
                .unwrap_or_else(|error| panic!("construct generated {} schema values: {error}", coverage.name));
            let generic_wire = encode_packet(Message::Schema(generic));
            let typed = decode_packet(&generic_wire);
            prop_assert_eq!(packet_message_id(&generic_wire), coverage.id, "generated {} packet ID", coverage.name);
            let typed_fields = schema_conforming::schema_conforming_fields(&typed);
            prop_assert_eq!(typed_fields.as_deref(), Some(expected_fields.as_slice()), "generated {} field mapping", coverage.name);

            let typed_wire = encode_packet(typed);
            prop_assert_eq!(&typed_wire, &generic_wire, "generated {} typed and schema wire packets", coverage.name);
        }
    }
}

proptest! {
    #[test]
    fn agent_update_schema_wire_matches(
        state in any::<u8>(),
        center in (-10_000.0f32..10_000.0, -10_000.0f32..10_000.0, -10_000.0f32..10_000.0),
        at in (-10_000.0f32..10_000.0, -10_000.0f32..10_000.0, -10_000.0f32..10_000.0),
        left in (-10_000.0f32..10_000.0, -10_000.0f32..10_000.0, -10_000.0f32..10_000.0),
        up in (-10_000.0f32..10_000.0, -10_000.0f32..10_000.0, -10_000.0f32..10_000.0),
        far in 0.0f32..10_000.0,
        control_flags in any::<u32>(),
        flags in any::<u8>(),
    ) {
        let agent = AgentUpdate {
            agent_data: agent_update::AgentData {
                agent_id: Uuid::from_bytes([1; 16]),
                session_id: Uuid::from_bytes([2; 16]),
                body_rotation: Quat::IDENTITY,
                head_rotation: Quat::from_xyzw(0.0, 0.0, 1.0, 0.0),
                state,
                camera_center: Vec3::new(center.0, center.1, center.2),
                camera_at_axis: Vec3::new(at.0, at.1, at.2),
                camera_left_axis: Vec3::new(left.0, left.1, left.2),
                camera_up_axis: Vec3::new(up.0, up.1, up.2),
                far,
                control_flags,
                flags,
            },
        };
        let expected_fields = schema_conforming::agent_update_fields(&agent);
        let typed = Message::AgentUpdate(agent);
        let message_schema = schema::find_by_id(AGENT_UPDATE).expect("AgentUpdate schema exists");
        let schema_message = SchemaMessage::new(message_schema, vec![BlockValues::single(expected_fields)])
            .expect("valid AgentUpdate schema values");
        let typed_wire = encode_packet(typed.clone());
        let schema_wire = encode_packet(Message::Schema(schema_message));
        prop_assert_eq!(&schema_wire, &typed_wire);
        prop_assert_eq!(decode_packet(&schema_wire), typed);
    }
}

proptest! {
    #[test]
    fn chat_from_viewer_schema_wire_matches(
        message in "[a-zA-Z0-9 ]{0,64}",
        chat_type in prop_oneof![
            Just(ChatType::Whisper),
            Just(ChatType::Normal),
            Just(ChatType::Shout),
            Just(ChatType::Say),
        ],
        channel in -10_000i32..10_000,
    ) {
        let typed = Message::ChatFromViewer(ChatFromViewer {
            agent_data: chat_from_viewer::AgentData {
                agent_id: Uuid::from_bytes([3; 16]),
                session_id: Uuid::from_bytes([4; 16]),
            },
            chat_data: chat_from_viewer::ChatData { message: message.clone(), chat_type, channel },
        });
        let mut encoded_chat = message.into_bytes();
        encoded_chat.push(0);
        let expected_blocks = vec![
            BlockValues::single(vec![FieldValue::Uuid(Uuid::from_bytes([3; 16])), FieldValue::Uuid(Uuid::from_bytes([4; 16]))]),
            BlockValues::single(vec![
                FieldValue::Bytes(encoded_chat),
                FieldValue::U8(chat_type.to_wire()),
                FieldValue::S32(channel),
            ]),
        ];
        let message_schema = schema::find_by_id(CHAT_FROM_VIEWER).expect("ChatFromViewer schema exists");
        let schema_message = SchemaMessage::new(message_schema, expected_blocks).expect("valid ChatFromViewer schema values");
        let typed_wire = encode_packet(typed.clone());
        let schema_wire = encode_packet(Message::Schema(schema_message));
        prop_assert_eq!(&schema_wire, &typed_wire);
        prop_assert_eq!(decode_packet(&schema_wire), typed);
    }
}

fn fields_in_schema_order(block: &schema::BlockSchema, expected: &[(&str, FieldValue)]) -> Vec<FieldValue> {
    assert_eq!(block.fields.len(), expected.len(), "expected values for every {} field", block.name);
    for (name, _) in expected {
        assert_eq!(block.fields.iter().filter(|field| field.name == *name).count(), 1, "{} has one {name} field", block.name);
    }

    block
        .fields
        .iter()
        .map(|field| {
            expected
                .iter()
                .find(|(name, _)| *name == field.name)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| panic!("{} has no expected value for field {}", block.name, field.name))
        })
        .collect()
}

fn c_string(value: &str) -> FieldValue {
    FieldValue::Bytes(value.bytes().chain([0]).collect())
}

#[test]
fn region_handshake_schema_wire_matches() {
    fn uuid(byte: u8) -> Uuid {
        Uuid::from_bytes([byte; 16])
    }

    let message_schema = schema::find_by_id(REGION_HANDSHAKE).expect("RegionHandshake schema exists");
    let region_info1 = RegionInfo1 {
        region_flags: 0x1020_3040,
        sim_access: 0x51,
        sim_name: "region name".into(),
        sim_owner: uuid(0x41),
        is_estate_manager: true,
        water_height: 12.5,
        billable_factor: 2.25,
        cache_id: uuid(0x42),
        terrain_base: [uuid(0x50), uuid(0x51), uuid(0x52), uuid(0x53)],
        terrain_detail: [uuid(0x60), uuid(0x61), uuid(0x62), uuid(0x63)],
        terrain_start_heights: [21.0, 22.0, 23.0, 24.0],
        terrain_height_ranges: [31.0, 32.0, 33.0, 34.0],
    };
    let region_info2 = RegionInfo2 { region_id: uuid(0x43) };
    let region_info3 = RegionInfo3 {
        cpu_class_id: -101,
        cpu_ratio: 202,
        colo_name: "colo".into(),
        product_sku: "sku-42".into(),
        product_name: "viewer product".into(),
    };
    let region_info4 = vec![
        RegionInfo4 { region_flags_extended: 0x0102_0304_0506_0708, region_protocols: 0x1112_1314_1516_1718 },
        RegionInfo4 { region_flags_extended: 0x2122_2324_2526_2728, region_protocols: 0x3132_3334_3536_3738 },
    ];
    let expected = RegionHandshake {
        region_info1: region_info1.clone(),
        region_info2: Some(region_info2.clone()),
        region_info3: Some(region_info3.clone()),
        region_info4: region_info4.clone(),
    };
    let blocks = vec![
        BlockValues::single(fields_in_schema_order(
            &message_schema.blocks[0],
            &[
                ("RegionFlags", FieldValue::U32(region_info1.region_flags)),
                ("SimAccess", FieldValue::U8(region_info1.sim_access)),
                ("SimName", c_string(&region_info1.sim_name)),
                ("SimOwner", FieldValue::Uuid(region_info1.sim_owner)),
                ("IsEstateManager", FieldValue::Bool(region_info1.is_estate_manager)),
                ("WaterHeight", FieldValue::F32(region_info1.water_height)),
                ("BillableFactor", FieldValue::F32(region_info1.billable_factor)),
                ("CacheID", FieldValue::Uuid(region_info1.cache_id)),
                ("TerrainBase0", FieldValue::Uuid(region_info1.terrain_base[0])),
                ("TerrainBase1", FieldValue::Uuid(region_info1.terrain_base[1])),
                ("TerrainBase2", FieldValue::Uuid(region_info1.terrain_base[2])),
                ("TerrainBase3", FieldValue::Uuid(region_info1.terrain_base[3])),
                ("TerrainDetail0", FieldValue::Uuid(region_info1.terrain_detail[0])),
                ("TerrainDetail1", FieldValue::Uuid(region_info1.terrain_detail[1])),
                ("TerrainDetail2", FieldValue::Uuid(region_info1.terrain_detail[2])),
                ("TerrainDetail3", FieldValue::Uuid(region_info1.terrain_detail[3])),
                ("TerrainStartHeight00", FieldValue::F32(region_info1.terrain_start_heights[0])),
                ("TerrainStartHeight01", FieldValue::F32(region_info1.terrain_start_heights[1])),
                ("TerrainStartHeight10", FieldValue::F32(region_info1.terrain_start_heights[2])),
                ("TerrainStartHeight11", FieldValue::F32(region_info1.terrain_start_heights[3])),
                ("TerrainHeightRange00", FieldValue::F32(region_info1.terrain_height_ranges[0])),
                ("TerrainHeightRange01", FieldValue::F32(region_info1.terrain_height_ranges[1])),
                ("TerrainHeightRange10", FieldValue::F32(region_info1.terrain_height_ranges[2])),
                ("TerrainHeightRange11", FieldValue::F32(region_info1.terrain_height_ranges[3])),
            ],
        )),
        BlockValues::single(fields_in_schema_order(&message_schema.blocks[1], &[("RegionID", FieldValue::Uuid(region_info2.region_id))])),
        BlockValues::single(fields_in_schema_order(
            &message_schema.blocks[2],
            &[
                ("CPUClassID", FieldValue::S32(region_info3.cpu_class_id)),
                ("CPURatio", FieldValue::S32(region_info3.cpu_ratio)),
                ("ColoName", c_string(&region_info3.colo_name)),
                ("ProductSKU", c_string(&region_info3.product_sku)),
                ("ProductName", c_string(&region_info3.product_name)),
            ],
        )),
        BlockValues::new(
            region_info4
                .iter()
                .map(|info| {
                    BlockInstance::new(fields_in_schema_order(
                        &message_schema.blocks[3],
                        &[
                            ("RegionFlagsExtended", FieldValue::U64(info.region_flags_extended)),
                            ("RegionProtocols", FieldValue::U64(info.region_protocols)),
                        ],
                    ))
                })
                .collect(),
        ),
    ];
    let typed = Message::RegionHandshake(expected);
    let schema_message = SchemaMessage::new(message_schema, blocks).expect("valid RegionHandshake schema values");
    let typed_wire = encode_packet(typed.clone());
    let schema_wire = encode_packet(Message::Schema(schema_message));

    assert_eq!(schema_wire, typed_wire, "typed and schema RegionHandshake wire layouts match");
    assert_eq!(decode_packet(&schema_wire), typed);
}
