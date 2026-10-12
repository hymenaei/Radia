use std::net::Ipv4Addr;

use math::Vec3;

use crate::{
    codec::{Reader, Writer},
    error::LLUdpError,
    messages::common::{encode_quantized, write_finite_vector3, write_finite_vector4},
    messages::{
        AgentMovementComplete, AgentUpdate, ChatFromSimulator, ChatFromViewer, CompleteAgentMovement, CompletePingCheck,
        ConfirmEnableSimulator, DisableSimulator, EnableSimulator, ImprovedTerseObjectUpdate, KillObject, LogoutReply, LogoutRequest,
        MapBlockReply, MapBlockRequest, ObjectUpdate, ObjectUpdateCached, ObjectUpdateCompressed, PacketAck, PingId, RegionHandleRequest,
        RegionHandshake, RegionHandshakeReply, RegionIdAndHandleReply, RequestMultipleObjects, StartPingCheck, UseCircuitCode,
    },
    messages::{
        agent_movement_complete, agent_update, chat_from_simulator, chat_from_viewer, complete_agent_movement, confirm_enable_simulator,
        enable_simulator, improved_terse_object_update, kill_object, logout_reply, logout_request, map_block_reply, map_block_request,
        object_update, object_update_cached, object_update_compressed, packet_ack, region_handle_request, region_handshake,
        region_handshake_reply, region_id_and_handle_reply, request_multiple_objects, use_circuit_code,
    },
    schema::{self, ids::*},
    schema_message::SchemaMessage,
    types::{Audible, ChatSourceType, ChatType, MessageId},
};

/// The typed LLUDP messages supported by this protocol subset.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    UseCircuitCode(UseCircuitCode),
    CompleteAgentMovement(CompleteAgentMovement),
    RegionHandshake(RegionHandshake),
    RegionHandshakeReply(RegionHandshakeReply),
    AgentMovementComplete(AgentMovementComplete),
    AgentUpdate(AgentUpdate),
    StartPingCheck(StartPingCheck),
    CompletePingCheck(CompletePingCheck),
    PacketAck(PacketAck),
    LogoutRequest(LogoutRequest),
    LogoutReply(LogoutReply),
    ChatFromViewer(ChatFromViewer),
    ChatFromSimulator(ChatFromSimulator),
    RegionHandleRequest(RegionHandleRequest),
    RegionIdAndHandleReply(RegionIdAndHandleReply),
    MapBlockRequest(MapBlockRequest),
    MapBlockReply(MapBlockReply),
    EnableSimulator(EnableSimulator),
    DisableSimulator(DisableSimulator),
    ConfirmEnableSimulator(ConfirmEnableSimulator),
    ObjectUpdate(ObjectUpdate),
    ObjectUpdateCompressed(ObjectUpdateCompressed),
    /// Partial object and avatar motion data; absent fields do not replace prior state.
    ImprovedTerseObjectUpdate(ImprovedTerseObjectUpdate),
    /// Cached object references that can trigger a full-data request on a cache miss.
    ObjectUpdateCached(ObjectUpdateCached),
    KillObject(KillObject),
    RequestMultipleObjects(RequestMultipleObjects),
    Schema(SchemaMessage),
    /// Unrecognized message ID and payload bytes retained without decoding.
    Unknown {
        id: MessageId,
        payload: Vec<u8>,
    },
}

impl From<AgentUpdate> for Message {
    fn from(message: AgentUpdate) -> Self {
        Self::AgentUpdate(message)
    }
}

impl From<ChatFromViewer> for Message {
    fn from(message: ChatFromViewer) -> Self {
        Self::ChatFromViewer(message)
    }
}

impl From<MapBlockRequest> for Message {
    fn from(message: MapBlockRequest) -> Self {
        Self::MapBlockRequest(message)
    }
}

impl From<RegionHandleRequest> for Message {
    fn from(message: RegionHandleRequest) -> Self {
        Self::RegionHandleRequest(message)
    }
}

impl From<RequestMultipleObjects> for Message {
    fn from(message: RequestMultipleObjects) -> Self {
        Self::RequestMultipleObjects(message)
    }
}

impl Message {
    pub fn is_inbound_only(&self) -> bool {
        matches!(
            self,
            Self::RegionHandshake(_)
                | Self::AgentMovementComplete(_)
                | Self::LogoutReply(_)
                | Self::ChatFromSimulator(_)
                | Self::RegionIdAndHandleReply(_)
                | Self::MapBlockReply(_)
                | Self::EnableSimulator(_)
                | Self::DisableSimulator(_)
                | Self::ConfirmEnableSimulator(_)
                | Self::ObjectUpdate(_)
                | Self::ObjectUpdateCompressed(_)
                | Self::ImprovedTerseObjectUpdate(_)
                | Self::ObjectUpdateCached(_)
                | Self::KillObject(_)
        )
    }

    pub(crate) fn id(&self) -> MessageId {
        match self {
            Self::UseCircuitCode(_) => USE_CIRCUIT_CODE,
            Self::CompleteAgentMovement(_) => COMPLETE_AGENT_MOVEMENT,
            Self::RegionHandshake(_) => REGION_HANDSHAKE,
            Self::RegionHandshakeReply(_) => REGION_HANDSHAKE_REPLY,
            Self::AgentMovementComplete(_) => AGENT_MOVEMENT_COMPLETE,
            Self::AgentUpdate(_) => AGENT_UPDATE,
            Self::StartPingCheck(_) => START_PING_CHECK,
            Self::CompletePingCheck(_) => COMPLETE_PING_CHECK,
            Self::PacketAck(_) => PACKET_ACK,
            Self::LogoutRequest(_) => LOGOUT_REQUEST,
            Self::LogoutReply(_) => LOGOUT_REPLY,
            Self::ChatFromViewer(_) => CHAT_FROM_VIEWER,
            Self::ChatFromSimulator(_) => CHAT_FROM_SIMULATOR,
            Self::RegionHandleRequest(_) => REGION_HANDLE_REQUEST,
            Self::RegionIdAndHandleReply(_) => REGION_ID_AND_HANDLE_REPLY,
            Self::MapBlockRequest(_) => MAP_BLOCK_REQUEST,
            Self::MapBlockReply(_) => MAP_BLOCK_REPLY,
            Self::EnableSimulator(_) => ENABLE_SIMULATOR,
            Self::DisableSimulator(_) => DISABLE_SIMULATOR,
            Self::ConfirmEnableSimulator(_) => CONFIRM_ENABLE_SIMULATOR,
            Self::ObjectUpdate(_) => OBJECT_UPDATE,
            Self::ObjectUpdateCompressed(_) => OBJECT_UPDATE_COMPRESSED,
            Self::ImprovedTerseObjectUpdate(_) => IMPROVED_TERSE_OBJECT_UPDATE,
            Self::ObjectUpdateCached(_) => OBJECT_UPDATE_CACHED,
            Self::KillObject(_) => KILL_OBJECT,
            Self::RequestMultipleObjects(_) => REQUEST_MULTIPLE_OBJECTS,
            Self::Schema(message) => message.id(),
            Self::Unknown { id, .. } => *id,
        }
    }

    pub(crate) fn default_zerocoded(&self) -> bool {
        if self.id() == CHAT_FROM_VIEWER {
            return false;
        }

        schema::find_by_id(self.id()).is_some_and(|metadata| metadata.encoding == schema::MessageEncoding::Zerocoded)
    }

    /// Whether this message requires a trusted circuit according to its template.
    pub fn requires_trusted_circuit(&self) -> bool {
        schema::find_by_id(self.id()).is_some_and(|metadata| metadata.trust == schema::MessageTrust::Trusted)
    }

    pub(crate) fn encode_payload(&self, output: &mut Vec<u8>, limit: usize) -> Result<(), LLUdpError> {
        let mut writer = Writer::with_limit(output, limit);
        match self {
            Self::UseCircuitCode(message) => {
                writer.u32(message.circuit_code.code);
                writer.uuid(message.circuit_code.session_id);
                writer.uuid(message.circuit_code.agent_id);
            }
            Self::CompleteAgentMovement(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.u32(message.agent_data.circuit_code);
            }
            Self::RegionHandshake(message) => {
                let info = &message.region_info1;
                writer.u32(info.region_flags);
                writer.u8(info.sim_access);
                writer.variable1_string(&info.sim_name)?;
                writer.uuid(info.sim_owner);
                writer.u8(u8::from(info.is_estate_manager));
                writer.f32(info.water_height);
                writer.f32(info.billable_factor);
                writer.uuid(info.cache_id);
                for id in info.terrain_base {
                    writer.uuid(id);
                }
                for id in info.terrain_detail {
                    writer.uuid(id);
                }
                for height in info.terrain_start_heights {
                    writer.f32(height);
                }
                for range in info.terrain_height_ranges {
                    writer.f32(range);
                }
                match (&message.region_info2, &message.region_info3) {
                    (Some(region_info2), Some(region_info3)) => {
                        writer.uuid(region_info2.region_id);
                        writer.i32(region_info3.cpu_class_id);
                        writer.i32(region_info3.cpu_ratio);
                        writer.variable1_string(&region_info3.colo_name)?;
                        writer.variable1_string(&region_info3.product_sku)?;
                        writer.variable1_string(&region_info3.product_name)?;
                        let count = u8::try_from(message.region_info4.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                        writer.u8(count);
                        for info in &message.region_info4 {
                            writer.u64(info.region_flags_extended);
                            writer.u64(info.region_protocols);
                        }
                    }
                    (None, None) if message.region_info4.is_empty() => {}
                    _ => return Err(LLUdpError::InvalidMessage),
                }
            }
            Self::RegionHandshakeReply(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.u32(message.region_info.flags);
            }
            Self::AgentMovementComplete(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.vector3(message.data.position);
                writer.vector3(message.data.look_at);
                writer.u64(message.data.region_handle);
                writer.u32(message.data.timestamp);
                writer.variable2_string(&message.sim_data.channel_version)?;
            }
            Self::AgentUpdate(message) => {
                let block = &message.agent_data;
                writer.uuid(block.agent_id);
                writer.uuid(block.session_id);
                writer.quaternion_xyz(block.body_rotation)?;
                writer.quaternion_xyz(block.head_rotation)?;
                writer.u8(block.state);
                writer.vector3(block.camera_center);
                writer.vector3(block.camera_at_axis);
                writer.vector3(block.camera_left_axis);
                writer.vector3(block.camera_up_axis);
                writer.f32(block.far);
                writer.u32(block.control_flags);
                writer.u8(block.flags);
            }
            Self::StartPingCheck(message) => {
                writer.u8(message.ping_id.ping_id);
                writer.u32(message.oldest_unacked);
            }
            Self::CompletePingCheck(message) => writer.u8(message.ping_id.ping_id),
            Self::PacketAck(message) => {
                let count = u8::try_from(message.packets.len()).map_err(|_| LLUdpError::TooManyAcks)?;
                writer.u8(count);
                for packet in &message.packets {
                    writer.u32(packet.id);
                }
            }
            Self::LogoutRequest(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
            }
            Self::LogoutReply(message) => {
                let count = u8::try_from(message.inventory_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.u8(count);
                for item in &message.inventory_data {
                    writer.uuid(item.item_id);
                }
            }
            Self::ChatFromViewer(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.variable2_string(&message.chat_data.message)?;
                writer.u8(message.chat_data.chat_type.to_wire());
                writer.i32(message.chat_data.channel);
            }
            Self::ChatFromSimulator(message) => {
                let block = &message.chat_data;
                writer.variable1_string(&block.from_name)?;
                writer.uuid(block.source_id);
                writer.uuid(block.owner_id);
                writer.u8(block.source_type.to_wire());
                writer.u8(block.chat_type.to_wire());
                writer.u8(block.audible.to_wire());
                writer.vector3(block.position);
                writer.variable2_string(&block.message)?;
            }
            Self::RegionHandleRequest(message) => writer.uuid(message.request_block.region_id),
            Self::RegionIdAndHandleReply(message) => {
                writer.uuid(message.reply_block.region_id);
                writer.u64(message.reply_block.region_handle);
            }
            Self::MapBlockRequest(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.u32(message.agent_data.flags);
                writer.u32(message.agent_data.estate_id);
                writer.u8(u8::from(message.agent_data.godlike));
                writer.u16(message.position_data.min_x);
                writer.u16(message.position_data.max_x);
                writer.u16(message.position_data.min_y);
                writer.u16(message.position_data.max_y);
            }
            Self::MapBlockReply(message) => {
                let count = u8::try_from(message.data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.uuid(message.agent_data.agent_id);
                writer.u32(message.agent_data.flags);
                writer.u8(count);
                for block in &message.data {
                    writer.u16(block.x);
                    writer.u16(block.y);
                    writer.variable1_string(&block.name)?;
                    writer.u8(block.access);
                    writer.u32(block.region_flags);
                    writer.u8(block.water_height);
                    writer.u8(block.agents);
                    writer.uuid(block.map_image_id);
                }
            }
            Self::EnableSimulator(message) => {
                writer.u64(message.simulator_info.handle);
                writer.bytes(&message.simulator_info.ip.octets());
                writer.bytes(&message.simulator_info.port.to_be_bytes());
            }
            Self::DisableSimulator(_) => {}
            Self::ConfirmEnableSimulator(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
            }
            Self::ObjectUpdate(message) => {
                message.region_data.encode(&mut writer);
                let count = u8::try_from(message.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for object in &message.objects {
                    object_update::encode_fields(object, &mut writer)?;
                }
            }
            Self::ObjectUpdateCompressed(message) => {
                message.region_data.encode(&mut writer);
                let count = u8::try_from(message.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for object in &message.objects {
                    writer.u32(object.update_flags);
                    object_update_compressed::encode_fields(object, &mut writer)?;
                }
            }
            Self::ImprovedTerseObjectUpdate(message) => {
                message.region_data.encode(&mut writer);
                let count = u8::try_from(message.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for object in &message.objects {
                    if object.avatar != object.collision_plane.is_some() {
                        return Err(LLUdpError::InvalidMessage);
                    }
                    let data_len = if object.collision_plane.is_some() { 60 } else { 44 };
                    writer.u8(data_len);
                    writer.u32(object.local_id);
                    writer.u8(object.state);
                    writer.u8(u8::from(object.avatar));
                    if let Some(plane) = object.collision_plane {
                        write_finite_vector4(&mut writer, plane)?;
                    }
                    write_finite_vector3(&mut writer, object.motion.position)?;
                    write_quantized_vector3(&mut writer, object.motion.velocity, -128.0, 128.0)?;
                    write_quantized_vector3(&mut writer, object.motion.acceleration, -64.0, 64.0)?;
                    for value in [object.motion.rotation.x, object.motion.rotation.y, object.motion.rotation.z, object.motion.rotation.w] {
                        writer.u16(encode_quantized(value, -1.0, 1.0)?);
                    }
                    write_quantized_vector3(&mut writer, object.motion.angular_velocity, -64.0, 64.0)?;
                    let texture_entry = object.texture_entry.as_slice();
                    let texture_len = u16::try_from(texture_entry.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                    writer.u16(texture_len);
                    writer.bytes(texture_entry);
                }
            }
            Self::ObjectUpdateCached(message) => {
                message.region_data.encode(&mut writer);
                let count = u8::try_from(message.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for object in &message.objects {
                    writer.u32(object.local_id);
                    writer.u32(object.crc);
                    writer.u32(object.update_flags);
                }
            }
            Self::KillObject(message) => {
                let count = u8::try_from(message.object_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for object in &message.object_data {
                    writer.u32(object.local_id);
                }
            }
            Self::RequestMultipleObjects(message) => {
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                let count = u8::try_from(message.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for object in &message.objects {
                    writer.u8(object.cache_miss_type.to_wire());
                    writer.u32(object.local_id);
                }
            }
            Self::Schema(message) => message.encode_payload(&mut writer)?,
            Self::Unknown { payload, .. } => writer.bytes(payload),
        }
        writer.finish()
    }

    pub(crate) fn decode(id: MessageId, payload: &[u8]) -> Result<(Self, Vec<u8>), LLUdpError> {
        let mut reader = Reader::new(payload);
        let message = match id {
            USE_CIRCUIT_CODE => Self::UseCircuitCode(UseCircuitCode {
                circuit_code: use_circuit_code::CircuitCode { code: reader.u32()?, session_id: reader.uuid()?, agent_id: reader.uuid()? },
            }),
            COMPLETE_AGENT_MOVEMENT => Self::CompleteAgentMovement(CompleteAgentMovement {
                agent_data: complete_agent_movement::AgentData {
                    agent_id: reader.uuid()?,
                    session_id: reader.uuid()?,
                    circuit_code: reader.u32()?,
                },
            }),
            REGION_HANDSHAKE => {
                let region_info1 = region_handshake::RegionInfo1 {
                    region_flags: reader.u32()?,
                    sim_access: reader.u8()?,
                    sim_name: reader.variable1_string()?,
                    sim_owner: reader.uuid()?,
                    is_estate_manager: reader.u8()? != 0,
                    water_height: reader.f32()?,
                    billable_factor: reader.f32()?,
                    cache_id: reader.uuid()?,
                    terrain_base: [reader.uuid()?, reader.uuid()?, reader.uuid()?, reader.uuid()?],
                    terrain_detail: [reader.uuid()?, reader.uuid()?, reader.uuid()?, reader.uuid()?],
                    terrain_start_heights: [reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?],
                    terrain_height_ranges: [reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?],
                };
                let (region_info2, region_info3, region_info4) = if reader.is_empty() {
                    (None, None, Vec::new())
                } else {
                    let region_info2 = region_handshake::RegionInfo2 { region_id: reader.uuid()? };
                    let region_info3 = region_handshake::RegionInfo3 {
                        cpu_class_id: reader.i32()?,
                        cpu_ratio: reader.i32()?,
                        colo_name: reader.variable1_string()?,
                        product_sku: reader.variable1_string()?,
                        product_name: reader.variable1_string()?,
                    };
                    let count = reader.terminal_variable_count()?;
                    let mut region_info4 = Vec::with_capacity(count);
                    for _ in 0..count {
                        region_info4
                            .push(region_handshake::RegionInfo4 { region_flags_extended: reader.u64()?, region_protocols: reader.u64()? });
                    }
                    (Some(region_info2), Some(region_info3), region_info4)
                };
                Self::RegionHandshake(RegionHandshake { region_info1, region_info2, region_info3, region_info4 })
            }
            REGION_HANDSHAKE_REPLY => Self::RegionHandshakeReply(RegionHandshakeReply {
                agent_data: region_handshake_reply::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
                region_info: region_handshake_reply::RegionInfo { flags: reader.u32()? },
            }),
            AGENT_MOVEMENT_COMPLETE => Self::AgentMovementComplete(AgentMovementComplete {
                agent_data: agent_movement_complete::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
                data: agent_movement_complete::Data {
                    position: reader.vector3()?,
                    look_at: reader.vector3()?,
                    region_handle: reader.u64()?,
                    timestamp: reader.u32()?,
                },
                sim_data: agent_movement_complete::SimData { channel_version: reader.variable2_string()? },
            }),
            AGENT_UPDATE => Self::AgentUpdate(AgentUpdate {
                agent_data: agent_update::AgentData {
                    agent_id: reader.uuid()?,
                    session_id: reader.uuid()?,
                    body_rotation: reader.quaternion()?,
                    head_rotation: reader.quaternion()?,
                    state: reader.u8()?,
                    camera_center: reader.vector3()?,
                    camera_at_axis: reader.vector3()?,
                    camera_left_axis: reader.vector3()?,
                    camera_up_axis: reader.vector3()?,
                    far: reader.f32()?,
                    control_flags: reader.u32()?,
                    flags: reader.u8()?,
                },
            }),
            START_PING_CHECK => {
                Self::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id: reader.u8()? }, oldest_unacked: reader.u32()? })
            }
            COMPLETE_PING_CHECK => Self::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: reader.u8()? } }),
            PACKET_ACK => {
                let count = reader.terminal_variable_count()?;
                let mut packets = Vec::with_capacity(count);
                for _ in 0..count {
                    packets.push(packet_ack::Packets { id: reader.u32()? });
                }
                Self::PacketAck(PacketAck { packets })
            }
            LOGOUT_REQUEST => Self::LogoutRequest(LogoutRequest {
                agent_data: logout_request::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            }),
            LOGOUT_REPLY => {
                let agent_data = logout_reply::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? };
                let count = reader.terminal_variable_count()?;
                let mut inventory_data = Vec::with_capacity(count);
                for _ in 0..count {
                    inventory_data.push(logout_reply::InventoryData { item_id: reader.uuid()? });
                }
                Self::LogoutReply(LogoutReply { agent_data, inventory_data })
            }
            CHAT_FROM_VIEWER => Self::ChatFromViewer(ChatFromViewer {
                agent_data: chat_from_viewer::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
                chat_data: chat_from_viewer::ChatData {
                    message: reader.variable2_string()?,
                    chat_type: ChatType::from_wire(reader.u8()?),
                    channel: reader.i32()?,
                },
            }),
            CHAT_FROM_SIMULATOR => Self::ChatFromSimulator(ChatFromSimulator {
                chat_data: chat_from_simulator::ChatData {
                    from_name: reader.variable1_string()?,
                    source_id: reader.uuid()?,
                    owner_id: reader.uuid()?,
                    source_type: ChatSourceType::from_wire(reader.u8()?),
                    chat_type: ChatType::from_wire(reader.u8()?),
                    audible: Audible::from_wire(reader.u8()?),
                    position: reader.vector3()?,
                    message: reader.variable2_string()?,
                },
            }),
            REGION_HANDLE_REQUEST => Self::RegionHandleRequest(region_handle_request::RegionHandleRequest {
                request_block: region_handle_request::RequestBlock { region_id: reader.uuid()? },
            }),
            REGION_ID_AND_HANDLE_REPLY => Self::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: region_id_and_handle_reply::ReplyBlock { region_id: reader.uuid()?, region_handle: reader.u64()? },
            }),
            MAP_BLOCK_REQUEST => Self::MapBlockRequest(map_block_request::MapBlockRequest {
                agent_data: map_block_request::AgentData {
                    agent_id: reader.uuid()?,
                    session_id: reader.uuid()?,
                    flags: reader.u32()?,
                    estate_id: reader.u32()?,
                    godlike: reader.u8()? != 0,
                },
                position_data: map_block_request::PositionData {
                    min_x: reader.u16()?,
                    max_x: reader.u16()?,
                    min_y: reader.u16()?,
                    max_y: reader.u16()?,
                },
            }),
            MAP_BLOCK_REPLY => {
                let agent_id = reader.uuid()?;
                let flags = reader.u32()?;
                let count = reader.terminal_variable_count()?;
                let mut blocks = Vec::with_capacity(count);
                for _ in 0..count {
                    blocks.push(map_block_reply::Data {
                        x: reader.u16()?,
                        y: reader.u16()?,
                        name: reader.variable1_string()?,
                        access: reader.u8()?,
                        region_flags: reader.u32()?,
                        water_height: reader.u8()?,
                        agents: reader.u8()?,
                        map_image_id: reader.uuid()?,
                    });
                }
                Self::MapBlockReply(MapBlockReply { agent_data: map_block_reply::AgentData { agent_id, flags }, data: blocks })
            }
            ENABLE_SIMULATOR => Self::EnableSimulator(EnableSimulator {
                simulator_info: enable_simulator::SimulatorInfo {
                    handle: reader.u64()?,
                    ip: Ipv4Addr::from([reader.u8()?, reader.u8()?, reader.u8()?, reader.u8()?]),
                    port: u16::from_be_bytes(reader.take(2)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?),
                },
            }),
            DISABLE_SIMULATOR => Self::DisableSimulator(DisableSimulator),
            CONFIRM_ENABLE_SIMULATOR => Self::ConfirmEnableSimulator(ConfirmEnableSimulator {
                agent_data: confirm_enable_simulator::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            }),
            OBJECT_UPDATE => Self::ObjectUpdate(object_update::decode(&mut reader)?),
            OBJECT_UPDATE_COMPRESSED => Self::ObjectUpdateCompressed(object_update_compressed::decode(&mut reader)?),
            IMPROVED_TERSE_OBJECT_UPDATE => Self::ImprovedTerseObjectUpdate(improved_terse_object_update::decode(&mut reader)?),
            OBJECT_UPDATE_CACHED => Self::ObjectUpdateCached(object_update_cached::decode(&mut reader)?),
            KILL_OBJECT => Self::KillObject(kill_object::decode(&mut reader)?),
            REQUEST_MULTIPLE_OBJECTS => Self::RequestMultipleObjects(request_multiple_objects::decode(&mut reader)?),
            _ => {
                if let Some(metadata) = schema::find_by_id(id) {
                    return Ok((Self::Schema(SchemaMessage::decode(metadata, payload)?), Vec::new()));
                }
                return Ok((Self::Unknown { id, payload: payload.to_vec() }, Vec::new()));
            }
        };
        Ok((message, reader.remaining().to_vec()))
    }
}

fn write_quantized_vector3(writer: &mut Writer<'_>, value: Vec3, lower: f32, upper: f32) -> Result<(), LLUdpError> {
    writer.u16(encode_quantized(value.x, lower, upper)?);
    writer.u16(encode_quantized(value.y, lower, upper)?);
    writer.u16(encode_quantized(value.z, lower, upper)?);
    Ok(())
}
