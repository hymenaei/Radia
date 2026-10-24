use std::assert_matches;

use protocol::{
    BlockInstance, BlockValues, FieldValue, SchemaMessage,
    lludp::{LLUdpError, Message, Packet},
    messages::{RegionInfo, TeleportFinish, region::region_info},
};
use uuid::Uuid;

fn schema_packet(name: &str, blocks: Vec<BlockValues>) -> Vec<u8> {
    let schema = SchemaMessage::schema_by_name(name).expect("schema exists in template");
    let message = SchemaMessage::new(schema, blocks).expect("schema fields match template");
    Packet::new(1, Message::Schema(message)).with_zero_coding(false).encode().expect("encode schema packet")
}

#[test]
fn partial_attachment_is_rejected() {
    let mut bytes = schema_packet(
        "AvatarAppearance",
        vec![
            BlockValues::single(vec![FieldValue::Uuid(Uuid::nil()), FieldValue::Bool(false)]),
            BlockValues::single(vec![FieldValue::Bytes(Vec::new())]),
            BlockValues::new(Vec::new()),
            BlockValues::new(Vec::new()),
            BlockValues::new(Vec::new()),
            BlockValues::new(vec![BlockInstance::new(vec![FieldValue::Uuid(Uuid::nil()), FieldValue::U8(0)])]),
        ],
    );
    bytes.pop();

    assert_eq!(Packet::decode(&bytes), Err(LLUdpError::TruncatedPacket));
}

#[test]
fn generic_preflight_rejects_truncation() {
    let mut bytes = schema_packet(
        "GenericMessage",
        vec![
            BlockValues::single(vec![FieldValue::Uuid(Uuid::nil()), FieldValue::Uuid(Uuid::nil()), FieldValue::Uuid(Uuid::nil())]),
            BlockValues::single(vec![FieldValue::Bytes(vec![b'm']), FieldValue::Uuid(Uuid::nil())]),
            BlockValues::new(vec![BlockInstance::new(vec![FieldValue::Bytes(vec![0])])]),
        ],
    );
    bytes.pop();

    assert_matches!(Packet::preflight(&bytes), Err(LLUdpError::TruncatedPacket));
}

#[test]
fn region_info_optional_tail_defaults() {
    let message = RegionInfo {
        agent_data: region_info::AgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        region_info: region_info::RegionInfoBlock {
            sim_name: Vec::new(),
            estate_id: Some(0),
            parent_estate_id: Some(0),
            region_flags: Some(0),
            sim_access: Some(0),
            max_agents: Some(0),
            billable_factor: Some(0.0),
            object_bonus_factor: Some(0.0),
            water_height: Some(0.0),
            terrain_raise_limit: Some(0.0),
            terrain_lower_limit: Some(0.0),
            price_per_meter: Some(0),
            redirect_grid_x: Some(0),
            redirect_grid_y: Some(0),
            use_estate_sun: Some(false),
            sun_hour: Some(0.0),
        },
        region_info2: region_info::RegionInfo2Block {
            product_sku: Vec::new(),
            product_name: Vec::new(),
            max_agents32: Some(0),
            hard_max_agents: Some(0),
            hard_max_objects: Some(0),
        },
        region_info3: Vec::new(),
        region_info5: Vec::new(),
        combat_settings: vec![region_info::CombatSettings {
            combat_flags: Some(0),
            on_death: Some(0),
            damage_throttle: Some(0.0),
            regeneration_rate: Some(0.0),
            invulnerability_time: Some(0.0),
            damage_limit: Some(23.0),
        }],
    };
    let mut bytes = Packet::new(1, Message::RegionInfo(message)).encode().expect("encode RegionInfo");
    bytes.truncate(bytes.len() - 4);

    let decoded = Packet::decode(&bytes).expect("decode truncated optional tail");
    let Message::RegionInfo(decoded) = decoded.into_message() else { panic!("expected RegionInfo") };
    assert_eq!(decoded.combat_settings[0].damage_limit, None);
}

#[test]
fn teleport_flags_accept_truncation() {
    let message = TeleportFinish {
        agent_id: Uuid::from_u128(1),
        location_id: 2,
        sim_ip: [192, 0, 2, 1],
        sim_port: [0x1f, 0x90],
        region_handle: 3,
        seed_capability: b"https://example.invalid/cap".to_vec(),
        sim_access: Some(13),
        teleport_flags: Some(4),
    };
    let mut bytes = Packet::new(1, Message::TeleportFinish(message)).encode().expect("encode TeleportFinish");
    bytes.truncate(bytes.len() - 4);

    let decoded = Packet::decode(&bytes).expect("decode missing optional flags");
    let Message::TeleportFinish(decoded) = decoded.into_message() else { panic!("expected TeleportFinish") };
    assert_eq!(decoded.sim_access, Some(13));
    assert_eq!(decoded.teleport_flags, None);
}
