use std::{mem::size_of, net::Ipv4Addr};

use math::Vec3;

use super::schema_message::{BlockInstance, FieldValue, SchemaMessage};
use super::{
    codec::{Reader, Writer},
    error::LLUdpError,
    types::{Audible, ChatSourceType, ChatType, MessageId},
};
use crate::{
    messages::object::encode_quantized,
    messages::{
        AgentMovementComplete, AgentThrottle, AgentUpdate, ChatFromSimulator, ChatFromViewer, CompleteAgentMovement, CompletePingCheck,
        ConfirmEnableSimulator, DisableSimulator, EnableSimulator, ExtraParamUpdate, GenericStreamingMessage, ImprovedTerseObjectUpdate,
        KillObject, LogoutReply, LogoutRequest, MapBlockReply, MapBlockRequest, ObjectExtraParams, ObjectUpdate, ObjectUpdateCached,
        ObjectUpdateCompressed, PacketAck, PingId, RegionHandleRequest, RegionHandshake, RegionHandshakeReply, RegionIdAndHandleReply,
        RequestMultipleObjects, StartPingCheck, TeleportCancel, UseCircuitCode,
    },
    messages::{
        agent::agent_movement_complete, agent::agent_throttle, agent::agent_update, agent::complete_agent_movement, agent::logout_reply,
        agent::logout_request, agent::teleport_cancel, chat::chat_from_simulator, chat::chat_from_viewer, circuit::packet_ack,
        circuit::use_circuit_code, generic_streaming_message, map::map_block_reply, map::map_block_request,
        object::improved_terse_object_update, object::kill_object, object::object_extra_params, object::object_update,
        object::object_update_cached, object::object_update_compressed, object::request_multiple_objects, region::confirm_enable_simulator,
        region::enable_simulator, region::region_handle_request, region::region_handshake, region::region_handshake_reply,
        region::region_id_and_handle_reply,
    },
    schema::{self, ids::*},
};

const _: () = {
    assert!(size_of::<region_handshake::RegionInfo4>() <= 512);
    assert!(size_of::<packet_ack::Packets>() <= 512);
    assert!(size_of::<logout_reply::InventoryData>() <= 512);
    assert!(size_of::<map_block_reply::Data>() <= 512);
    assert!(size_of::<crate::messages::object::ExtraParameter>() <= 32);
    assert!(size_of::<object_update::ObjectData>() <= 512);
    assert!(size_of::<object_update_compressed::ObjectData>() <= 512);
    assert!(size_of::<improved_terse_object_update::ObjectData>() <= 512);
    assert!(size_of::<improved_terse_object_update::ObjectBlock>() <= 512);
    assert!(size_of::<object_update_cached::CachedObjectReference>() <= 512);
    assert!(size_of::<kill_object::ObjectData>() <= 512);
    assert!(size_of::<request_multiple_objects::ObjectRequest>() <= 512);
    assert!(size_of::<object_extra_params::ExtraParamUpdate>() <= 512);
    assert!(size_of::<packet_ack::Packets>() <= size_of::<BlockInstance>() + size_of::<FieldValue>());
    assert!(size_of::<logout_reply::InventoryData>() <= size_of::<BlockInstance>() + size_of::<FieldValue>());
    assert!(size_of::<map_block_reply::Data>() <= size_of::<BlockInstance>() + 9 * size_of::<FieldValue>());
    assert!(size_of::<object_update_cached::CachedObjectReference>() <= size_of::<BlockInstance>() + 3 * size_of::<FieldValue>());
    assert!(size_of::<kill_object::ObjectData>() <= size_of::<BlockInstance>() + size_of::<FieldValue>());
    assert!(size_of::<request_multiple_objects::ObjectRequest>() <= size_of::<BlockInstance>() + 2 * size_of::<FieldValue>());
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationStrategy {
    NoAlloc,
    SchemaDerived,
    Specialized,
    Opaque,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TypedMessageId {
    UseCircuitCode,
    CompleteAgentMovement,
    RegionHandshake,
    RegionHandshakeReply,
    AgentMovementComplete,
    AgentUpdate,
    AgentThrottle,
    StartPingCheck,
    CompletePingCheck,
    PacketAck,
    TeleportCancel,
    LogoutRequest,
    LogoutReply,
    ChatFromViewer,
    ChatFromSimulator,
    RegionHandleRequest,
    RegionIdAndHandleReply,
    MapBlockRequest,
    MapBlockReply,
    EnableSimulator,
    DisableSimulator,
    ConfirmEnableSimulator,
    ObjectUpdate,
    ObjectUpdateCompressed,
    ImprovedTerseObjectUpdate,
    ObjectUpdateCached,
    KillObject,
    RequestMultipleObjects,
    ObjectExtraParams,
    GenericStreamingMessage,
}

impl TypedMessageId {
    fn from_id(id: MessageId) -> Option<Self> {
        Some(match id {
            USE_CIRCUIT_CODE => Self::UseCircuitCode,
            COMPLETE_AGENT_MOVEMENT => Self::CompleteAgentMovement,
            REGION_HANDSHAKE => Self::RegionHandshake,
            REGION_HANDSHAKE_REPLY => Self::RegionHandshakeReply,
            AGENT_MOVEMENT_COMPLETE => Self::AgentMovementComplete,
            AGENT_UPDATE => Self::AgentUpdate,
            AGENT_THROTTLE => Self::AgentThrottle,
            START_PING_CHECK => Self::StartPingCheck,
            COMPLETE_PING_CHECK => Self::CompletePingCheck,
            PACKET_ACK => Self::PacketAck,
            TELEPORT_CANCEL => Self::TeleportCancel,
            LOGOUT_REQUEST => Self::LogoutRequest,
            LOGOUT_REPLY => Self::LogoutReply,
            CHAT_FROM_VIEWER => Self::ChatFromViewer,
            CHAT_FROM_SIMULATOR => Self::ChatFromSimulator,
            REGION_HANDLE_REQUEST => Self::RegionHandleRequest,
            REGION_ID_AND_HANDLE_REPLY => Self::RegionIdAndHandleReply,
            MAP_BLOCK_REQUEST => Self::MapBlockRequest,
            MAP_BLOCK_REPLY => Self::MapBlockReply,
            ENABLE_SIMULATOR => Self::EnableSimulator,
            DISABLE_SIMULATOR => Self::DisableSimulator,
            CONFIRM_ENABLE_SIMULATOR => Self::ConfirmEnableSimulator,
            OBJECT_UPDATE => Self::ObjectUpdate,
            OBJECT_UPDATE_COMPRESSED => Self::ObjectUpdateCompressed,
            IMPROVED_TERSE_OBJECT_UPDATE => Self::ImprovedTerseObjectUpdate,
            OBJECT_UPDATE_CACHED => Self::ObjectUpdateCached,
            KILL_OBJECT => Self::KillObject,
            REQUEST_MULTIPLE_OBJECTS => Self::RequestMultipleObjects,
            OBJECT_EXTRA_PARAMS => Self::ObjectExtraParams,
            GENERIC_STREAMING_MESSAGE => Self::GenericStreamingMessage,
            _ => return None,
        })
    }

    const fn allocation_strategy(self) -> AllocationStrategy {
        match self {
            Self::UseCircuitCode
            | Self::CompleteAgentMovement
            | Self::RegionHandshakeReply
            | Self::AgentUpdate
            | Self::AgentThrottle
            | Self::StartPingCheck
            | Self::CompletePingCheck
            | Self::LogoutRequest
            | Self::RegionHandleRequest
            | Self::RegionIdAndHandleReply
            | Self::MapBlockRequest
            | Self::EnableSimulator
            | Self::DisableSimulator
            | Self::ConfirmEnableSimulator => AllocationStrategy::NoAlloc,
            Self::RegionHandshake
            | Self::ObjectUpdate
            | Self::ObjectUpdateCompressed
            | Self::ImprovedTerseObjectUpdate
            | Self::ObjectExtraParams
            | Self::GenericStreamingMessage => AllocationStrategy::Specialized,
            Self::TeleportCancel
            | Self::AgentMovementComplete
            | Self::PacketAck
            | Self::LogoutReply
            | Self::ChatFromViewer
            | Self::ChatFromSimulator
            | Self::MapBlockReply
            | Self::ObjectUpdateCached
            | Self::KillObject
            | Self::RequestMultipleObjects => AllocationStrategy::SchemaDerived,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    UseCircuitCode(UseCircuitCode),
    CompleteAgentMovement(CompleteAgentMovement),
    RegionHandshake(RegionHandshake),
    RegionHandshakeReply(RegionHandshakeReply),
    AgentMovementComplete(AgentMovementComplete),
    AgentUpdate(AgentUpdate),
    AgentThrottle(AgentThrottle),
    StartPingCheck(StartPingCheck),
    CompletePingCheck(CompletePingCheck),
    PacketAck(PacketAck),
    TeleportCancel(TeleportCancel),
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
    ImprovedTerseObjectUpdate(ImprovedTerseObjectUpdate),
    ObjectUpdateCached(ObjectUpdateCached),
    KillObject(KillObject),
    RequestMultipleObjects(RequestMultipleObjects),
    ObjectExtraParams(ObjectExtraParams),
    GenericStreamingMessage(GenericStreamingMessage),
    Schema(SchemaMessage),
    Unknown { id: MessageId, payload: Vec<u8> },
}

impl From<AgentUpdate> for Message {
    fn from(message: AgentUpdate) -> Self {
        Self::AgentUpdate(message)
    }
}

impl From<AgentThrottle> for Message {
    fn from(message: AgentThrottle) -> Self {
        Self::AgentThrottle(message)
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

impl From<TeleportCancel> for Message {
    fn from(message: TeleportCancel) -> Self {
        Self::TeleportCancel(message)
    }
}

impl From<RequestMultipleObjects> for Message {
    fn from(message: RequestMultipleObjects) -> Self {
        Self::RequestMultipleObjects(message)
    }
}

impl From<ObjectExtraParams> for Message {
    fn from(message: ObjectExtraParams) -> Self {
        Self::ObjectExtraParams(message)
    }
}

impl From<GenericStreamingMessage> for Message {
    fn from(message: GenericStreamingMessage) -> Self {
        Self::GenericStreamingMessage(message)
    }
}

impl Message {
    pub fn allocation_strategy(id: MessageId) -> AllocationStrategy {
        if let Some(typed_id) = TypedMessageId::from_id(id) {
            return typed_id.allocation_strategy();
        }
        if schema::find_by_id(id).is_some() { AllocationStrategy::SchemaDerived } else { AllocationStrategy::Opaque }
    }

    pub fn typed_allocation_strategy(id: MessageId) -> Option<AllocationStrategy> {
        TypedMessageId::from_id(id).map(TypedMessageId::allocation_strategy)
    }

    pub(crate) fn allocation_bound(id: MessageId, payload: &[u8]) -> Result<(AllocationStrategy, usize), LLUdpError> {
        match Self::allocation_strategy(id) {
            AllocationStrategy::NoAlloc | AllocationStrategy::Opaque => Ok((Self::allocation_strategy(id), payload.len())),
            AllocationStrategy::SchemaDerived => {
                let metadata = schema::find_by_id(id).ok_or(LLUdpError::InvalidMessageId)?;
                Ok((AllocationStrategy::SchemaDerived, SchemaMessage::allocation_bound(metadata, payload)?))
            }
            AllocationStrategy::Specialized => Ok((AllocationStrategy::Specialized, specialized_allocation_bound(id, payload)?)),
        }
    }

    pub(crate) fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        match self {
            Self::RegionHandshake(message) => {
                add_capacity(&mut bytes, message.region_info4.capacity(), size_of::<region_handshake::RegionInfo4>());
                add_string_capacity(&mut bytes, &message.region_info1.sim_name);
                if let Some(info) = &message.region_info3 {
                    add_string_capacity(&mut bytes, &info.colo_name);
                    add_string_capacity(&mut bytes, &info.product_sku);
                    add_string_capacity(&mut bytes, &info.product_name);
                }
            }
            Self::AgentMovementComplete(message) => add_string_capacity(&mut bytes, &message.sim_data.channel_version),
            Self::PacketAck(message) => add_capacity(&mut bytes, message.packets.capacity(), size_of::<packet_ack::Packets>()),
            Self::LogoutReply(message) => {
                add_capacity(&mut bytes, message.inventory_data.capacity(), size_of::<logout_reply::InventoryData>());
            }
            Self::ChatFromViewer(message) => add_string_capacity(&mut bytes, &message.chat_data.message),
            Self::ChatFromSimulator(message) => {
                add_string_capacity(&mut bytes, &message.chat_data.from_name);
                add_string_capacity(&mut bytes, &message.chat_data.message);
            }
            Self::MapBlockReply(message) => {
                add_capacity(&mut bytes, message.data.capacity(), size_of::<map_block_reply::Data>());
                for block in &message.data {
                    add_string_capacity(&mut bytes, &block.name);
                }
            }
            Self::ObjectUpdate(message) => {
                add_capacity(&mut bytes, message.objects.capacity(), size_of::<object_update::ObjectData>());
                for object in &message.objects {
                    add_object_update_capacity(&mut bytes, object);
                }
            }
            Self::ObjectUpdateCompressed(message) => {
                add_capacity(&mut bytes, message.objects.capacity(), size_of::<object_update_compressed::ObjectData>());
                for object in &message.objects {
                    add_object_update_compressed_capacity(&mut bytes, object);
                }
            }
            Self::ImprovedTerseObjectUpdate(message) => {
                add_capacity(&mut bytes, message.objects.capacity(), size_of::<improved_terse_object_update::ObjectBlock>());
                for object in &message.objects {
                    match object {
                        improved_terse_object_update::ObjectBlock::Decoded(object) => {
                            add_capacity(&mut bytes, object.texture_entry.capacity(), size_of::<u8>());
                        }
                        improved_terse_object_update::ObjectBlock::Malformed { packed_data, texture_entry } => {
                            add_capacity(&mut bytes, packed_data.capacity(), size_of::<u8>());
                            add_capacity(&mut bytes, texture_entry.capacity(), size_of::<u8>());
                        }
                    }
                }
            }
            Self::ObjectUpdateCached(message) => {
                add_capacity(&mut bytes, message.objects.capacity(), size_of::<object_update_cached::CachedObjectReference>());
            }
            Self::KillObject(message) => add_capacity(&mut bytes, message.object_data.capacity(), size_of::<kill_object::ObjectData>()),
            Self::RequestMultipleObjects(message) => {
                add_capacity(&mut bytes, message.objects.capacity(), size_of::<request_multiple_objects::ObjectRequest>());
            }
            Self::ObjectExtraParams(message) => {
                add_capacity(&mut bytes, message.object_data.capacity(), size_of::<ExtraParamUpdate>());
                for update in &message.object_data {
                    add_capacity(&mut bytes, update.data.capacity(), size_of::<u8>());
                }
            }
            Self::GenericStreamingMessage(message) => add_capacity(&mut bytes, message.data.capacity(), size_of::<u8>()),
            Self::Schema(message) => add_schema_capacity(&mut bytes, message),
            Self::Unknown { payload, .. } => add_capacity(&mut bytes, payload.capacity(), size_of::<u8>()),
            Self::UseCircuitCode(_)
            | Self::CompleteAgentMovement(_)
            | Self::RegionHandshakeReply(_)
            | Self::AgentUpdate(_)
            | Self::AgentThrottle(_)
            | Self::StartPingCheck(_)
            | Self::CompletePingCheck(_)
            | Self::TeleportCancel(_)
            | Self::LogoutRequest(_)
            | Self::RegionHandleRequest(_)
            | Self::RegionIdAndHandleReply(_)
            | Self::MapBlockRequest(_)
            | Self::EnableSimulator(_)
            | Self::DisableSimulator(_)
            | Self::ConfirmEnableSimulator(_) => {}
        }
        bytes
    }

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
                | Self::ObjectExtraParams(_)
        )
    }

    pub fn name(&self) -> &'static str {
        schema::find_by_id(self.id()).map_or("Unknown", |message| message.name)
    }

    pub(crate) fn id(&self) -> MessageId {
        match self {
            Self::UseCircuitCode(_) => USE_CIRCUIT_CODE,
            Self::CompleteAgentMovement(_) => COMPLETE_AGENT_MOVEMENT,
            Self::RegionHandshake(_) => REGION_HANDSHAKE,
            Self::RegionHandshakeReply(_) => REGION_HANDSHAKE_REPLY,
            Self::AgentMovementComplete(_) => AGENT_MOVEMENT_COMPLETE,
            Self::AgentUpdate(_) => AGENT_UPDATE,
            Self::AgentThrottle(_) => AGENT_THROTTLE,
            Self::StartPingCheck(_) => START_PING_CHECK,
            Self::CompletePingCheck(_) => COMPLETE_PING_CHECK,
            Self::PacketAck(_) => PACKET_ACK,
            Self::TeleportCancel(_) => TELEPORT_CANCEL,
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
            Self::ObjectExtraParams(_) => OBJECT_EXTRA_PARAMS,
            Self::GenericStreamingMessage(_) => GENERIC_STREAMING_MESSAGE,
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
            Self::AgentThrottle(message) => {
                let rates = message.throttle.throttles;
                if !rates.is_valid() {
                    return Err(LLUdpError::InvalidMessage);
                }
                writer.uuid(message.agent_data.agent_id);
                writer.uuid(message.agent_data.session_id);
                writer.u32(message.agent_data.circuit_code);
                writer.u32(message.throttle.gen_counter);
                writer.u8(28);
                for rate in rates.wire_order() {
                    writer.f32(rate);
                }
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
            Self::TeleportCancel(message) => teleport_cancel::encode(message, &mut writer),
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
                for block in &message.objects {
                    match block {
                        improved_terse_object_update::ObjectBlock::Decoded(object) => {
                            if object.avatar != object.collision_plane.is_some() {
                                return Err(LLUdpError::InvalidMessage);
                            }
                            let data_len = if object.collision_plane.is_some() { 60 } else { 44 };
                            writer.u8(data_len);
                            writer.u32(object.local_id);
                            writer.u8(object.state);
                            writer.u8(u8::from(object.avatar));
                            if let Some(plane) = object.collision_plane {
                                writer.finite_vector4(plane)?;
                            }
                            writer.finite_vector3(object.motion.position)?;
                            write_quantized_vector3(&mut writer, object.motion.velocity, -128.0, 128.0)?;
                            write_quantized_vector3(&mut writer, object.motion.acceleration, -64.0, 64.0)?;
                            for value in
                                [object.motion.rotation.x, object.motion.rotation.y, object.motion.rotation.z, object.motion.rotation.w]
                            {
                                writer.u16(encode_quantized(value, -1.0, 1.0)?);
                            }
                            write_quantized_vector3(&mut writer, object.motion.angular_velocity, -64.0, 64.0)?;
                            let texture_entry = object.texture_entry.as_slice();
                            let texture_len = u16::try_from(texture_entry.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                            writer.u16(texture_len);
                            writer.bytes(texture_entry);
                        }
                        improved_terse_object_update::ObjectBlock::Malformed { packed_data, texture_entry } => {
                            writer.u8(u8::try_from(packed_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
                            writer.bytes(packed_data);
                            writer.u16(u16::try_from(texture_entry.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
                            writer.bytes(texture_entry);
                        }
                    }
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
            Self::ObjectExtraParams(message) => object_extra_params::encode(message, &mut writer)?,
            Self::GenericStreamingMessage(message) => generic_streaming_message::encode(message, &mut writer)?,
            Self::Schema(message) => message.encode_payload(&mut writer)?,
            Self::Unknown { payload, .. } => writer.bytes(payload),
        }
        writer.finish()
    }

    pub(crate) fn decode(id: MessageId, payload: &[u8]) -> Result<(Self, Vec<u8>), LLUdpError> {
        let mut reader = Reader::new(payload);
        let message = match TypedMessageId::from_id(id) {
            Some(TypedMessageId::UseCircuitCode) => Self::UseCircuitCode(UseCircuitCode {
                circuit_code: use_circuit_code::CircuitCode { code: reader.u32()?, session_id: reader.uuid()?, agent_id: reader.uuid()? },
            }),
            Some(TypedMessageId::CompleteAgentMovement) => Self::CompleteAgentMovement(CompleteAgentMovement {
                agent_data: complete_agent_movement::AgentData {
                    agent_id: reader.uuid()?,
                    session_id: reader.uuid()?,
                    circuit_code: reader.u32()?,
                },
            }),
            Some(TypedMessageId::RegionHandshake) => {
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
            Some(TypedMessageId::RegionHandshakeReply) => Self::RegionHandshakeReply(RegionHandshakeReply {
                agent_data: region_handshake_reply::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
                region_info: region_handshake_reply::RegionInfo { flags: reader.u32()? },
            }),
            Some(TypedMessageId::AgentMovementComplete) => Self::AgentMovementComplete(AgentMovementComplete {
                agent_data: agent_movement_complete::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
                data: agent_movement_complete::Data {
                    position: reader.vector3()?,
                    look_at: reader.vector3()?,
                    region_handle: reader.u64()?,
                    timestamp: reader.u32()?,
                },
                sim_data: agent_movement_complete::SimData { channel_version: reader.variable2_string()? },
            }),
            Some(TypedMessageId::AgentUpdate) => Self::AgentUpdate(AgentUpdate {
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
            Some(TypedMessageId::AgentThrottle) => {
                let agent_data =
                    agent_throttle::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()?, circuit_code: reader.u32()? };
                let gen_counter = reader.u32()?;
                let byte_count = usize::from(reader.u8()?);
                if byte_count != 7 * size_of::<f32>() {
                    return Err(LLUdpError::InvalidMessage);
                }
                let mut rates = [0.0; 7];
                for rate in &mut rates {
                    *rate = reader.f32()?;
                }
                let throttles = agent_throttle::ThrottleRates::from_wire_order(rates);
                if !throttles.is_valid() {
                    return Err(LLUdpError::InvalidMessage);
                }
                Self::AgentThrottle(AgentThrottle { agent_data, throttle: agent_throttle::Throttle { gen_counter, throttles } })
            }
            Some(TypedMessageId::StartPingCheck) => {
                Self::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id: reader.u8()? }, oldest_unacked: reader.u32()? })
            }
            Some(TypedMessageId::CompletePingCheck) => {
                Self::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: reader.u8()? } })
            }
            Some(TypedMessageId::PacketAck) => {
                let count = reader.terminal_variable_count()?;
                let mut packets = Vec::with_capacity(count);
                for _ in 0..count {
                    packets.push(packet_ack::Packets { id: reader.u32()? });
                }
                Self::PacketAck(PacketAck { packets })
            }
            Some(TypedMessageId::TeleportCancel) => Self::TeleportCancel(teleport_cancel::decode(&mut reader)?),
            Some(TypedMessageId::LogoutRequest) => Self::LogoutRequest(LogoutRequest {
                agent_data: logout_request::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            }),
            Some(TypedMessageId::LogoutReply) => {
                let agent_data = logout_reply::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? };
                let count = reader.terminal_variable_count()?;
                let mut inventory_data = Vec::with_capacity(count);
                for _ in 0..count {
                    inventory_data.push(logout_reply::InventoryData { item_id: reader.uuid()? });
                }
                Self::LogoutReply(LogoutReply { agent_data, inventory_data })
            }
            Some(TypedMessageId::ChatFromViewer) => Self::ChatFromViewer(ChatFromViewer {
                agent_data: chat_from_viewer::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
                chat_data: chat_from_viewer::ChatData {
                    message: reader.variable2_string()?,
                    chat_type: ChatType::from_wire(reader.u8()?),
                    channel: reader.i32()?,
                },
            }),
            Some(TypedMessageId::ChatFromSimulator) => Self::ChatFromSimulator(ChatFromSimulator {
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
            Some(TypedMessageId::RegionHandleRequest) => Self::RegionHandleRequest(region_handle_request::RegionHandleRequest {
                request_block: region_handle_request::RequestBlock { region_id: reader.uuid()? },
            }),
            Some(TypedMessageId::RegionIdAndHandleReply) => Self::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: region_id_and_handle_reply::ReplyBlock { region_id: reader.uuid()?, region_handle: reader.u64()? },
            }),
            Some(TypedMessageId::MapBlockRequest) => Self::MapBlockRequest(map_block_request::MapBlockRequest {
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
            Some(TypedMessageId::MapBlockReply) => {
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
            Some(TypedMessageId::EnableSimulator) => Self::EnableSimulator(EnableSimulator {
                simulator_info: enable_simulator::SimulatorInfo {
                    handle: reader.u64()?,
                    ip: Ipv4Addr::from([reader.u8()?, reader.u8()?, reader.u8()?, reader.u8()?]),
                    port: u16::from_be_bytes(reader.take(2)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?),
                },
            }),
            Some(TypedMessageId::DisableSimulator) => Self::DisableSimulator(DisableSimulator),
            Some(TypedMessageId::ConfirmEnableSimulator) => Self::ConfirmEnableSimulator(ConfirmEnableSimulator {
                agent_data: confirm_enable_simulator::AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            }),
            Some(TypedMessageId::ObjectUpdate) => Self::ObjectUpdate(object_update::decode(&mut reader)?),
            Some(TypedMessageId::ObjectUpdateCompressed) => Self::ObjectUpdateCompressed(object_update_compressed::decode(&mut reader)?),
            Some(TypedMessageId::ImprovedTerseObjectUpdate) => {
                Self::ImprovedTerseObjectUpdate(improved_terse_object_update::decode(&mut reader)?)
            }
            Some(TypedMessageId::ObjectUpdateCached) => Self::ObjectUpdateCached(object_update_cached::decode(&mut reader)?),
            Some(TypedMessageId::KillObject) => Self::KillObject(kill_object::decode(&mut reader)?),
            Some(TypedMessageId::RequestMultipleObjects) => Self::RequestMultipleObjects(request_multiple_objects::decode(&mut reader)?),
            Some(TypedMessageId::ObjectExtraParams) => Self::ObjectExtraParams(object_extra_params::decode(&mut reader)?),
            Some(TypedMessageId::GenericStreamingMessage) => Self::GenericStreamingMessage(generic_streaming_message::decode(&mut reader)?),
            None => {
                if let Some(metadata) = schema::find_by_id(id) {
                    return Ok((Self::Schema(SchemaMessage::decode(metadata, payload)?), Vec::new()));
                }
                return Ok((Self::Unknown { id, payload: payload.to_vec() }, Vec::new()));
            }
        };
        Ok((message, reader.remaining().to_vec()))
    }
}

fn specialized_allocation_bound(id: MessageId, payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut allocation_bytes = payload.len();

    match id {
        REGION_HANDSHAKE => {
            let mut reader = Reader::new(payload);
            reader.take(4)?;
            reader.take(1)?;
            skip_variable1(&mut reader)?;
            reader.take(16)?;
            reader.take(1)?;
            reader.take(8)?;
            reader.take(16)?;
            reader.take(128)?;
            reader.take(32)?;
            if !reader.is_empty() {
                reader.take(16)?;
                reader.take(8)?;
                skip_variable1(&mut reader)?;
                skip_variable1(&mut reader)?;
                skip_variable1(&mut reader)?;
                let count = reader.terminal_variable_count()?;
                add_allocation_bytes(&mut allocation_bytes, allocation_product(count, size_of::<region_handshake::RegionInfo4>())?)?;
            }
        }
        OBJECT_UPDATE => {
            add_allocation_bytes(&mut allocation_bytes, object_update::preflight_allocation_bytes(payload)?)?;
        }
        OBJECT_UPDATE_COMPRESSED => {
            add_allocation_bytes(&mut allocation_bytes, object_update_compressed::preflight_allocation_bytes(payload)?)?;
        }
        IMPROVED_TERSE_OBJECT_UPDATE => {
            add_allocation_bytes(&mut allocation_bytes, improved_terse_object_update::preflight_allocation_bytes(payload)?)?;
        }
        OBJECT_EXTRA_PARAMS => {
            add_allocation_bytes(&mut allocation_bytes, object_extra_params::preflight_allocation_bytes(payload)?)?;
        }
        GENERIC_STREAMING_MESSAGE => {
            add_allocation_bytes(&mut allocation_bytes, generic_streaming_message::preflight_allocation_bytes(payload)?)?;
        }
        _ => return Err(LLUdpError::InvalidMessageId),
    }

    Ok(allocation_bytes)
}

fn skip_variable1(reader: &mut Reader<'_>) -> Result<(), LLUdpError> {
    let length = usize::from(reader.u8()?);
    reader.take(length)?;
    Ok(())
}

fn allocation_product(left: usize, right: usize) -> Result<usize, LLUdpError> {
    left.checked_mul(right).ok_or(LLUdpError::AllocationBoundOverflow)
}

fn add_allocation_bytes(total: &mut usize, bytes: usize) -> Result<(), LLUdpError> {
    *total = total.checked_add(bytes).ok_or(LLUdpError::AllocationBoundOverflow)?;
    Ok(())
}

fn write_quantized_vector3(writer: &mut Writer<'_>, value: Vec3, lower: f32, upper: f32) -> Result<(), LLUdpError> {
    writer.u16(encode_quantized(value.x, lower, upper)?);
    writer.u16(encode_quantized(value.y, lower, upper)?);
    writer.u16(encode_quantized(value.z, lower, upper)?);
    Ok(())
}

fn add_capacity(total: &mut usize, capacity: usize, element_size: usize) {
    *total = total.saturating_add(capacity.saturating_mul(element_size));
}

fn add_string_capacity(total: &mut usize, value: &String) {
    add_capacity(total, value.capacity(), size_of::<u8>());
}

fn add_extra_parameter_capacity(total: &mut usize, parameters: &Vec<crate::messages::object::ExtraParameter>) {
    add_capacity(total, parameters.capacity(), size_of::<crate::messages::object::ExtraParameter>());
    for parameter in parameters {
        add_capacity(total, parameter.data.capacity(), size_of::<u8>());
    }
}

fn add_object_update_capacity(total: &mut usize, object: &object_update::ObjectData) {
    add_capacity(total, object.texture_entry.capacity(), size_of::<u8>());
    add_capacity(total, object.texture_anim.capacity(), size_of::<u8>());
    add_capacity(total, object.name_value.capacity(), size_of::<u8>());
    add_capacity(total, object.data.capacity(), size_of::<u8>());
    add_string_capacity(total, &object.text.text);
    add_string_capacity(total, &object.media_url);
    add_capacity(total, object.particle_system.capacity(), size_of::<u8>());
    add_extra_parameter_capacity(total, &object.extra_parameters);
}

fn add_object_update_compressed_capacity(total: &mut usize, object: &object_update_compressed::ObjectData) {
    if let Some(object_update_compressed::ObjectSpecificData::ScratchPad(bytes)) = &object.data {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    if let Some(text) = &object.text {
        add_string_capacity(total, &text.text);
    }
    if let Some(media_url) = &object.media_url {
        add_string_capacity(total, media_url);
    }
    if let Some(bytes) = &object.legacy_particle_system {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    add_extra_parameter_capacity(total, &object.extra_parameters);
    if let Some(name_value) = &object.name_value {
        add_string_capacity(total, name_value);
    }
    if let Some(bytes) = &object.texture_entry {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    if let Some(bytes) = &object.texture_animation {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
    if let Some(bytes) = &object.particle_system {
        add_capacity(total, bytes.capacity(), size_of::<u8>());
    }
}

fn add_schema_capacity(total: &mut usize, message: &SchemaMessage) {
    *total = total.saturating_add(message.retained_capacity_bytes());
}

#[cfg(test)]
mod tests {
    use crate::lludp::Packet;
    use proptest::{prelude::*, test_runner::Config};
    use test_support::{lludp::vectors, parse_hex_fixture};

    use super::*;

    struct TypedMessageFixture {
        name: &'static str,
        id: MessageId,
        fixture: &'static str,
        schema_conforming: bool,
    }

    macro_rules! conformance_category {
        (SchemaConforming) => {
            true
        };
        ($other:ident) => {
            false
        };
    }

    macro_rules! typed_message_coverage {
        ($(
            $name:literal => $_variant:ident, $id:ident, $fixture:expr,
            $_preflight:ident, $conformance:ident, [
                $(($_block:literal, $_mode:expr, $_fields:literal)),* $(,)?
            ];
        )+) => {
            const TYPED_MESSAGE_FIXTURES: &[TypedMessageFixture] = &[
                $(TypedMessageFixture {
                    name: $name,
                    id: $id,
                    fixture: $fixture,
                    schema_conforming: conformance_category!($conformance),
                }),+
            ];
        };
    }

    include!("../../tests/support/typed_coverage.rs");

    mod schema_conforming {
        use crate as protocol;

        include!("../../tests/support/schema_conforming.rs");
    }

    mod schema_values {
        use crate as protocol;

        include!("../../tests/support/schema_values.rs");
    }

    #[test]
    fn typed_and_schema_routes_are_explicit() {
        for (id, strategy) in [
            (USE_CIRCUIT_CODE, AllocationStrategy::NoAlloc),
            (COMPLETE_AGENT_MOVEMENT, AllocationStrategy::NoAlloc),
            (REGION_HANDSHAKE, AllocationStrategy::Specialized),
            (AGENT_MOVEMENT_COMPLETE, AllocationStrategy::SchemaDerived),
            (PACKET_ACK, AllocationStrategy::SchemaDerived),
            (OBJECT_UPDATE, AllocationStrategy::Specialized),
        ] {
            assert_eq!(Message::typed_allocation_strategy(id), Some(strategy));
            assert_eq!(Message::allocation_strategy(id), strategy);
        }

        let schema_only_id = schema::find_by_name("NeighborList").expect("schema-only message exists").id;
        assert_eq!(TypedMessageId::from_id(schema_only_id), None);
        assert_eq!(Message::typed_allocation_strategy(schema_only_id), None);
        assert_eq!(Message::allocation_strategy(schema_only_id), AllocationStrategy::SchemaDerived);

        let unknown_id = MessageId::high(u32::from(u16::MAX));
        assert!(schema::find_by_id(unknown_id).is_none());
        assert_eq!(TypedMessageId::from_id(unknown_id), None);
        assert_eq!(Message::allocation_strategy(unknown_id), AllocationStrategy::Opaque);
    }

    #[test]
    fn every_specialized_message_has_a_preflight_route() {
        for metadata in schema::MESSAGES.iter() {
            if Message::allocation_strategy(metadata.id) == AllocationStrategy::Specialized {
                assert_ne!(
                    specialized_allocation_bound(metadata.id, &[]),
                    Err(LLUdpError::InvalidMessageId),
                    "{} must have an explicit specialized allocation bound",
                    metadata.name
                );
            }
        }
    }

    #[test]
    fn typed_payloads_match_schema_fields() {
        let mut checked = 0;

        for coverage in TYPED_MESSAGE_FIXTURES.iter().filter(|coverage| coverage.schema_conforming) {
            let bytes = parse_hex_fixture(coverage.fixture).expect("typed fixture is valid hex");
            let packet = Packet::decode(&bytes).unwrap_or_else(|error| panic!("decode {} fixture: {error}", coverage.name));
            let message = packet.into_message();
            assert_eq!(message.id(), coverage.id, "fixture message ID for {}", coverage.name);
            let expected_fields = schema_conforming::schema_conforming_fields(&message)
                .unwrap_or_else(|| panic!("{} has a schema-conforming field mapping", coverage.name));

            let mut typed_payload = Vec::new();
            message
                .encode_payload(&mut typed_payload, usize::MAX)
                .unwrap_or_else(|error| panic!("encode typed {} fixture: {error}", coverage.name));
            let metadata = schema::find_by_id(coverage.id).expect("inventory ID has generated schema");
            let generic = SchemaMessage::decode(metadata, &typed_payload)
                .unwrap_or_else(|error| panic!("decode typed {} fixture as schema: {error}", coverage.name));
            assert_eq!(generic.blocks(), expected_fields, "fixture field meaning and order for {}", coverage.name);
            assert!(generic.trailing_bytes().is_empty(), "typed {} fixture has no generic trailing bytes", coverage.name);
            checked += 1;
        }

        assert_eq!(checked, TYPED_MESSAGE_FIXTURES.iter().filter(|coverage| coverage.schema_conforming).count());
    }

    proptest! {
        #![proptest_config(Config { cases: 32, ..Config::default() })]

        #[test]
        fn typed_encoding_matches_generic_schema(seed in any::<u64>()) {
            for coverage in TYPED_MESSAGE_FIXTURES.iter().filter(|coverage| coverage.schema_conforming) {
                let metadata = schema::find_by_id(coverage.id).expect("typed inventory message has generated schema");
                let fields = schema_values::generated_schema_blocks(metadata, seed);
                let generic = SchemaMessage::new(metadata, fields.clone())
                    .unwrap_or_else(|error| panic!("construct generated {} schema values: {error}", coverage.name));
                let mut generic_payload = Vec::new();
                Message::Schema(generic)
                    .encode_payload(&mut generic_payload, usize::MAX)
                    .unwrap_or_else(|error| panic!("encode generated {} generic payload: {error}", coverage.name));

                let (typed, trailing) = Message::decode(coverage.id, &generic_payload)
                    .unwrap_or_else(|error| panic!("decode generated {} typed payload: {error}", coverage.name));
                prop_assert!(trailing.is_empty(), "generated {} payload has no typed trailing bytes", coverage.name);
                prop_assert_eq!(typed.id(), coverage.id, "generated {} typed ID", coverage.name);
                let mut typed_payload = Vec::new();
                typed
                    .encode_payload(&mut typed_payload, usize::MAX)
                    .unwrap_or_else(|error| panic!("encode generated {} typed payload: {error}", coverage.name));
                let decoded = SchemaMessage::decode(metadata, &typed_payload)
                    .unwrap_or_else(|error| panic!("decode generated {} typed payload as schema: {error}", coverage.name));
                prop_assert_eq!(decoded.blocks(), fields.as_slice(), "generated {} typed-to-generic fields", coverage.name);
                prop_assert!(decoded.trailing_bytes().is_empty(), "generated {} typed payload has no generic trailing bytes", coverage.name);
            }
        }
    }
}
