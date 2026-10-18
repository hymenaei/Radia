use super::parse_hex_fixture;

/// Hex-encoded LLUDP packets used by tests.
pub mod vectors {
    pub const USE_CIRCUIT_CODE: &str = include_str!("../fixtures/lludp/use_circuit_code.hex");
    /// Current-template handshake with no `RegionInfo4` entries.
    pub const REGION_HANDSHAKE: &str = include_str!("../fixtures/lludp/region_handshake.hex");
    /// `RegionHandshake` packet with two `RegionInfo4` entries.
    pub const REGION_HANDSHAKE_INFO4: &str = include_str!("../fixtures/lludp/region_handshake_info4.hex");
    pub const REGION_HANDSHAKE_STRINGS: &str = include_str!("../fixtures/lludp/region_handshake_strings.hex");
    /// `RegionHandshake` packet whose region name contains invalid UTF-8.
    pub const REGION_HANDSHAKE_INVALID_UTF8: &str = include_str!("../fixtures/lludp/region_handshake_invalid_utf8.hex");
    /// Legacy `RegionHandshake` packet containing only the original region block.
    pub const REGION_HANDSHAKE_LEGACY: &str = include_str!("../fixtures/lludp/region_handshake_legacy.hex");
    pub const REGION_HANDSHAKE_REPLY: &str = include_str!("../fixtures/lludp/region_handshake_reply.hex");
    pub const AGENT_MOVEMENT_COMPLETE: &str = include_str!("../fixtures/lludp/agent_movement_complete.hex");
    pub const COMPLETE_AGENT_MOVEMENT: &str = include_str!("../fixtures/lludp/complete_agent_movement.hex");
    pub const COMPLETE_PING_CHECK: &str = include_str!("../fixtures/lludp/complete_ping_check.hex");
    pub const TELEPORT_CANCEL: &str = include_str!("../fixtures/lludp/teleport_cancel.hex");
    pub const LOGOUT_REQUEST: &str = include_str!("../fixtures/lludp/logout_request.hex");
    pub const LOGOUT_REPLY: &str = include_str!("../fixtures/lludp/logout_reply.hex");
    /// `LogoutReply` packet with its terminal inventory count omitted.
    pub const LOGOUT_REPLY_EMPTY: &str = include_str!("../fixtures/lludp/logout_reply_empty.hex");
    pub const LOGOUT_REPLY_ZERO_COUNT: &str = include_str!("../fixtures/lludp/logout_reply_zero_count.hex");
    pub const LOGOUT_REPLY_NIL_ITEM: &str = include_str!("../fixtures/lludp/logout_reply_nil_item.hex");
    pub const CHAT_FROM_VIEWER: &str = include_str!("../fixtures/lludp/chat_from_viewer.hex");
    pub const CHAT_FROM_VIEWER_NIL_IDS: &str = include_str!("../fixtures/lludp/chat_from_viewer_nil_ids.hex");
    pub const CHAT_FROM_VIEWER_ZEROCODED: &str = include_str!("../fixtures/lludp/chat_from_viewer_zerocoded.hex");
    pub const CHAT_FROM_SIMULATOR: &str = include_str!("../fixtures/lludp/chat_from_simulator.hex");
    pub const CHAT_FROM_SIMULATOR_UTF8: &str = include_str!("../fixtures/lludp/chat_from_simulator_utf8.hex");
    /// `ChatFromSimulator` packet with invalid bytes after embedded NULs.
    pub const CHAT_FROM_SIMULATOR_EMBEDDED_NUL: &str = include_str!("../fixtures/lludp/chat_from_simulator_embedded_nul.hex");
    pub const CHAT_FROM_SIMULATOR_INVALID_UTF8_MESSAGE: &str =
        include_str!("../fixtures/lludp/chat_from_simulator_invalid_utf8_message.hex");
    pub const CHAT_FROM_SIMULATOR_INVALID_UTF8_NAME: &str = include_str!("../fixtures/lludp/chat_from_simulator_invalid_utf8_name.hex");
    pub const CHAT_FROM_SIMULATOR_INAUDIBLE_EMPTY: &str = include_str!("../fixtures/lludp/chat_from_simulator_inaudible_empty.hex");
    pub const PACKET_ACK: &str = include_str!("../fixtures/lludp/packet_ack.hex");
    pub const APPENDED_ACKS: &str = include_str!("../fixtures/lludp/appended_acks.hex");
    pub const START_PING_CHECK: &str = include_str!("../fixtures/lludp/start_ping_check.hex");
    pub const MAP_BLOCK_REQUEST: &str = include_str!("../fixtures/lludp/map_block_request.hex");
    pub const MAP_BLOCK_REPLY: &str = include_str!("../fixtures/lludp/map_block_reply.hex");
    pub const COARSE_LOCATION_UPDATE: &str = include_str!("../fixtures/lludp/coarse_location_update.hex");
    pub const REGION_HANDLE_REQUEST: &str = include_str!("../fixtures/lludp/region_handle_request.hex");
    pub const REGION_ID_AND_HANDLE_REPLY: &str = include_str!("../fixtures/lludp/region_id_and_handle_reply.hex");
    pub const ENABLE_SIMULATOR: &str = include_str!("../fixtures/lludp/enable_simulator.hex");
    pub const DISABLE_SIMULATOR: &str = include_str!("../fixtures/lludp/disable_simulator.hex");
    pub const CONFIRM_ENABLE_SIMULATOR: &str = include_str!("../fixtures/lludp/confirm_enable_simulator.hex");
    pub const AGENT_UPDATE_ZEROCODED: &str = include_str!("../fixtures/lludp/agent_update_zerocoded.hex");
    /// Reliable, zero-coded initial `AgentUpdate` packet with default camera values.
    pub const AGENT_UPDATE_INITIAL: &str = include_str!("../fixtures/lludp/agent_update_initial.hex");
    /// Unzero-coded `AgentThrottle` packet with distinct rates in wire order.
    pub const AGENT_THROTTLE: &str = include_str!("../fixtures/lludp/agent_throttle.hex");
    /// Over-limit zero-coded `TransferRequest` packet for decoded-size rejection.
    pub const TRANSFER_REQUEST_LARGE_ZEROCODED: &str = include_str!("../fixtures/lludp/transfer_request_large_zerocoded.hex");
    /// Unknown-message packet whose decoded size is exactly 8 KiB.
    pub const ZEROCODE_PACKET_AT_LIMIT: &str = include_str!("../fixtures/lludp/zerocode_packet_at_limit.hex");
    /// `ObjectUpdateCompressed` packet with a truncated packed object identity.
    pub const OBJECT_UPDATE_COMPRESSED: &str = include_str!("../fixtures/lludp/object_update_compressed.hex");
    pub const OBJECT_UPDATE: &str = include_str!("../fixtures/lludp/object_update.hex");
    /// Compressed volume object with typed optional fields and volume data.
    pub const OBJECT_UPDATE_COMPRESSED_TYPED: &str = include_str!("../fixtures/lludp/object_update_compressed_typed.hex");
    pub const IMPROVED_TERSE_OBJECT_UPDATE: &str = include_str!("../fixtures/lludp/improved_terse_object_update.hex");
    /// Avatar `ImprovedTerseObjectUpdate` packet with a collision plane.
    pub const IMPROVED_TERSE_OBJECT_UPDATE_AVATAR: &str = include_str!("../fixtures/lludp/improved_terse_object_update_avatar.hex");
    pub const OBJECT_UPDATE_CACHED: &str = include_str!("../fixtures/lludp/object_update_cached.hex");
    /// Synthetic `ObjectExtraParams` packet with one in-use parameter block.
    pub const OBJECT_EXTRA_PARAMS: &str = include_str!("../fixtures/lludp/object_extra_params.hex");
    /// Synthetic `GenericStreamingMessage` packet containing a method and two payload bytes.
    pub const GENERIC_STREAMING_MESSAGE: &str = include_str!("../fixtures/lludp/generic_streaming_message.hex");
    pub const KILL_OBJECT: &str = include_str!("../fixtures/lludp/kill_object.hex");
    pub const REQUEST_MULTIPLE_OBJECTS: &str = include_str!("../fixtures/lludp/request_multiple_objects.hex");
    /// `UUIDNameRequest` packet with its final variable-block count omitted.
    pub const UUID_NAME_REQUEST_EMPTY: &str = include_str!("../fixtures/lludp/uuid_name_request_empty.hex");
    pub const ZEROCODE_EXTENDED_RUN: &str = include_str!("../fixtures/lludp/zerocode_extended_run.hex");

    /// All checked-in LLUDP fixtures as `(file name, source text)` pairs.
    pub const ALL: &[(&str, &str)] = &[
        ("use_circuit_code.hex", USE_CIRCUIT_CODE),
        ("region_handshake.hex", REGION_HANDSHAKE),
        ("region_handshake_info4.hex", REGION_HANDSHAKE_INFO4),
        ("region_handshake_strings.hex", REGION_HANDSHAKE_STRINGS),
        ("region_handshake_invalid_utf8.hex", REGION_HANDSHAKE_INVALID_UTF8),
        ("region_handshake_legacy.hex", REGION_HANDSHAKE_LEGACY),
        ("region_handshake_reply.hex", REGION_HANDSHAKE_REPLY),
        ("agent_movement_complete.hex", AGENT_MOVEMENT_COMPLETE),
        ("agent_throttle.hex", AGENT_THROTTLE),
        ("complete_agent_movement.hex", COMPLETE_AGENT_MOVEMENT),
        ("complete_ping_check.hex", COMPLETE_PING_CHECK),
        ("teleport_cancel.hex", TELEPORT_CANCEL),
        ("logout_request.hex", LOGOUT_REQUEST),
        ("logout_reply.hex", LOGOUT_REPLY),
        ("logout_reply_empty.hex", LOGOUT_REPLY_EMPTY),
        ("logout_reply_zero_count.hex", LOGOUT_REPLY_ZERO_COUNT),
        ("logout_reply_nil_item.hex", LOGOUT_REPLY_NIL_ITEM),
        ("chat_from_viewer.hex", CHAT_FROM_VIEWER),
        ("chat_from_viewer_nil_ids.hex", CHAT_FROM_VIEWER_NIL_IDS),
        ("chat_from_viewer_zerocoded.hex", CHAT_FROM_VIEWER_ZEROCODED),
        ("chat_from_simulator.hex", CHAT_FROM_SIMULATOR),
        ("chat_from_simulator_utf8.hex", CHAT_FROM_SIMULATOR_UTF8),
        ("chat_from_simulator_embedded_nul.hex", CHAT_FROM_SIMULATOR_EMBEDDED_NUL),
        ("chat_from_simulator_invalid_utf8_message.hex", CHAT_FROM_SIMULATOR_INVALID_UTF8_MESSAGE),
        ("chat_from_simulator_invalid_utf8_name.hex", CHAT_FROM_SIMULATOR_INVALID_UTF8_NAME),
        ("chat_from_simulator_inaudible_empty.hex", CHAT_FROM_SIMULATOR_INAUDIBLE_EMPTY),
        ("packet_ack.hex", PACKET_ACK),
        ("appended_acks.hex", APPENDED_ACKS),
        ("start_ping_check.hex", START_PING_CHECK),
        ("map_block_request.hex", MAP_BLOCK_REQUEST),
        ("map_block_reply.hex", MAP_BLOCK_REPLY),
        ("coarse_location_update.hex", COARSE_LOCATION_UPDATE),
        ("region_handle_request.hex", REGION_HANDLE_REQUEST),
        ("region_id_and_handle_reply.hex", REGION_ID_AND_HANDLE_REPLY),
        ("enable_simulator.hex", ENABLE_SIMULATOR),
        ("disable_simulator.hex", DISABLE_SIMULATOR),
        ("confirm_enable_simulator.hex", CONFIRM_ENABLE_SIMULATOR),
        ("agent_update_zerocoded.hex", AGENT_UPDATE_ZEROCODED),
        ("agent_update_initial.hex", AGENT_UPDATE_INITIAL),
        ("transfer_request_large_zerocoded.hex", TRANSFER_REQUEST_LARGE_ZEROCODED),
        ("zerocode_packet_at_limit.hex", ZEROCODE_PACKET_AT_LIMIT),
        ("object_update_compressed.hex", OBJECT_UPDATE_COMPRESSED),
        ("object_update.hex", OBJECT_UPDATE),
        ("object_update_compressed_typed.hex", OBJECT_UPDATE_COMPRESSED_TYPED),
        ("improved_terse_object_update.hex", IMPROVED_TERSE_OBJECT_UPDATE),
        ("improved_terse_object_update_avatar.hex", IMPROVED_TERSE_OBJECT_UPDATE_AVATAR),
        ("object_update_cached.hex", OBJECT_UPDATE_CACHED),
        ("object_extra_params.hex", OBJECT_EXTRA_PARAMS),
        ("generic_streaming_message.hex", GENERIC_STREAMING_MESSAGE),
        ("kill_object.hex", KILL_OBJECT),
        ("request_multiple_objects.hex", REQUEST_MULTIPLE_OBJECTS),
        ("uuid_name_request_empty.hex", UUID_NAME_REQUEST_EMPTY),
        ("zerocode_extended_run.hex", ZEROCODE_EXTENDED_RUN),
    ];

    /// Looks up a fixture by its exact file name, including the `.hex` suffix.
    ///
    /// Returns the fixture's source text, or `None` when no name matches.
    pub fn get(name: &str) -> Option<&'static str> {
        ALL.iter().find_map(|(fixture_name, contents)| (*fixture_name == name).then_some(*contents))
    }
}

/// Parses a checked-in LLUDP fixture by its exact file name.
///
/// # Panics
///
/// Panics if no fixture has that name or if the fixture contains invalid hexadecimal data.
pub fn fixture(name: &str) -> Vec<u8> {
    let source = vectors::get(name).unwrap_or_else(|| panic!("unknown fixture: {name}"));
    parse_hex_fixture(source).unwrap_or_else(|error| panic!("invalid fixture {name}: {error}"))
}
