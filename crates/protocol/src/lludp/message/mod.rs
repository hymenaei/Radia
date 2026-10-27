pub(crate) mod allocation;

use std::mem::size_of;

use super::{
    codec::{Reader, Writer},
    error::LLUdpError,
    schema_message::SchemaMessage,
    types::MessageId,
};
use crate::schema;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationStrategy {
    NoAlloc,
    SchemaDerived,
    Specialized,
    Opaque,
}

pub(crate) trait TypedMessage: Sized {
    const ID: MessageId;
    const ALLOCATION: AllocationStrategy;
    const INBOUND_ONLY: bool;
    const ZEROCODED: Option<bool> = None;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError>;
    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError>;

    fn preflight_allocation_bound(payload: &[u8]) -> Result<usize, LLUdpError> {
        Ok(payload.len())
    }

    fn retained_capacity_bytes(&self) -> usize {
        0
    }
}

macro_rules! messages {
    ($($message:ident($message_type:path)),* $(,)?) => {
        #[derive(Clone, Debug, PartialEq)]
        pub enum Message {
            $($message($message_type),)*
            Schema(SchemaMessage),
            Unknown { id: MessageId, payload: Vec<u8> },
        }

        impl Message {
            pub fn allocation_strategy(id: MessageId) -> AllocationStrategy {
                Self::typed_allocation_strategy(id).unwrap_or_else(|| {
                    if schema::find_by_id(id).is_some() {
                        AllocationStrategy::SchemaDerived
                    } else {
                        AllocationStrategy::Opaque
                    }
                })
            }

            pub fn typed_allocation_strategy(id: MessageId) -> Option<AllocationStrategy> {
                match id {
                    $(<$message_type as TypedMessage>::ID => Some(<$message_type as TypedMessage>::ALLOCATION),)*
                    _ => None,
                }
            }

            pub fn is_inbound_only(&self) -> bool {
                match self {
                    $(Self::$message(_) => <$message_type as TypedMessage>::INBOUND_ONLY,)*
                    Self::Schema(_) | Self::Unknown { .. } => false,
                }
            }

            pub fn name(&self) -> &'static str {
                schema::find_by_id(self.id()).map_or("Unknown", |message| message.name)
            }

            pub(crate) fn id(&self) -> MessageId {
                match self {
                    $(Self::$message(_) => <$message_type as TypedMessage>::ID,)*
                    Self::Schema(message) => message.id(),
                    Self::Unknown { id, .. } => *id,
                }
            }

            pub(crate) fn decode(id: MessageId, payload: &[u8]) -> Result<(Self, Vec<u8>), LLUdpError> {
                let mut reader = Reader::new(payload);
                let message = match id {
                    $(<$message_type as TypedMessage>::ID => Self::$message(<$message_type as TypedMessage>::decode(&mut reader)?),)*
                    _ => {
                        if let Some(metadata) = schema::find_by_id(id) {
                            return Ok((Self::Schema(SchemaMessage::decode(metadata, payload)?), Vec::new()));
                        }
                        return Ok((Self::Unknown { id, payload: payload.to_vec() }, Vec::new()));
                    }
                };
                Ok((message, reader.remaining().to_vec()))
            }

            pub(crate) fn encode_payload(&self, output: &mut Vec<u8>, limit: usize) -> Result<(), LLUdpError> {
                let mut writer = Writer::with_limit(output, limit);
                match self {
                    $(Self::$message(message) => <$message_type as TypedMessage>::encode(message, &mut writer)?,)*
                    Self::Schema(message) => message.encode_payload(&mut writer)?,
                    Self::Unknown { payload, .. } => writer.bytes(payload),
                }
                writer.finish()
            }

            pub(crate) fn allocation_bound(id: MessageId, payload: &[u8]) -> Result<(AllocationStrategy, usize), LLUdpError> {
                match id {
                    $(<$message_type as TypedMessage>::ID => allocation::typed_allocation_bound::<$message_type>(payload),)*
                    _ => allocation::fallback_allocation_bound(id, payload),
                }
            }

            pub(crate) fn retained_capacity_bytes(&self) -> usize {
                match self {
                    $(Self::$message(message) => <$message_type as TypedMessage>::retained_capacity_bytes(message),)*
                    Self::Schema(message) => message.retained_capacity_bytes(),
                    Self::Unknown { payload, .. } => allocation::capacity_bytes(payload.capacity(), size_of::<u8>()),
                }
            }

            fn zerocoded_override(id: MessageId) -> Option<bool> {
                match id {
                    $(<$message_type as TypedMessage>::ID => <$message_type as TypedMessage>::ZEROCODED,)*
                    _ => None,
                }
            }

            pub(crate) fn default_zerocoded(&self) -> bool {
                Self::zerocoded_override(self.id()).unwrap_or_else(|| {
                    schema::find_by_id(self.id()).is_some_and(|metadata| metadata.encoding == schema::MessageEncoding::Zerocoded)
                })
            }

            pub fn requires_trusted_circuit(&self) -> bool {
                schema::find_by_id(self.id()).is_some_and(|metadata| metadata.trust == schema::MessageTrust::Trusted)
            }
        }

        $(
            impl From<$message_type> for Message {
                fn from(message: $message_type) -> Self {
                    Self::$message(message)
                }
            }
        )*
    };
}

messages! {
    UseCircuitCode(crate::messages::UseCircuitCode),
    CompleteAgentMovement(crate::messages::CompleteAgentMovement),
    RegionHandshake(crate::messages::RegionHandshake),
    RegionHandshakeReply(crate::messages::RegionHandshakeReply),
    AgentMovementComplete(crate::messages::AgentMovementComplete),
    AgentRequestSit(crate::messages::AgentRequestSit),
    AgentSit(crate::messages::AgentSit),
    AgentUpdate(crate::messages::AgentUpdate),
    SetAlwaysRun(crate::messages::SetAlwaysRun),
    AgentThrottle(crate::messages::AgentThrottle),
    StartPingCheck(crate::messages::StartPingCheck),
    CompletePingCheck(crate::messages::CompletePingCheck),
    PacketAck(crate::messages::PacketAck),
    TeleportCancel(crate::messages::TeleportCancel),
    LogoutRequest(crate::messages::LogoutRequest),
    LogoutReply(crate::messages::LogoutReply),
    ChatFromViewer(crate::messages::ChatFromViewer),
    ChatFromSimulator(crate::messages::ChatFromSimulator),
    RegionHandleRequest(crate::messages::RegionHandleRequest),
    RegionIdAndHandleReply(crate::messages::RegionIdAndHandleReply),
    MapBlockRequest(crate::messages::MapBlockRequest),
    MapBlockReply(crate::messages::MapBlockReply),
    EnableSimulator(crate::messages::EnableSimulator),
    DisableSimulator(crate::messages::DisableSimulator),
    ConfirmEnableSimulator(crate::messages::ConfirmEnableSimulator),
    ObjectUpdate(crate::messages::ObjectUpdate),
    ObjectUpdateCompressed(crate::messages::ObjectUpdateCompressed),
    ImprovedTerseObjectUpdate(crate::messages::ImprovedTerseObjectUpdate),
    ObjectUpdateCached(crate::messages::ObjectUpdateCached),
    KillObject(crate::messages::KillObject),
    RequestMultipleObjects(crate::messages::RequestMultipleObjects),
    ObjectExtraParams(crate::messages::ObjectExtraParams),
    GenericStreamingMessage(crate::messages::GenericStreamingMessage),
    AvatarAppearance(crate::messages::AvatarAppearance),
    AvatarSitResponse(crate::messages::AvatarSitResponse),
    UpdateCreateInventoryItem(crate::messages::UpdateCreateInventoryItem),
    RemoveInventoryItem(crate::messages::RemoveInventoryItem),
    RemoveInventoryFolder(crate::messages::RemoveInventoryFolder),
    RemoveInventoryObjects(crate::messages::RemoveInventoryObjects),
    SaveAssetIntoInventory(crate::messages::SaveAssetIntoInventory),
    BulkUpdateInventory(crate::messages::BulkUpdateInventory),
    MoveInventoryItem(crate::messages::MoveInventoryItem),
    MoveInventoryFolder(crate::messages::MoveInventoryFolder),
    ParcelProperties(crate::messages::ParcelProperties),
    ParcelOverlay(crate::messages::ParcelOverlay),
    GenericMessage(crate::messages::GenericMessage),
    LayerData(crate::messages::LayerData),
    RegionInfo(crate::messages::RegionInfo),
    TeleportFinish(crate::messages::TeleportFinish),
    TeleportFailed(crate::messages::TeleportFailed),
}

#[cfg(test)]
mod tests {
    use super::allocation::fallback_allocation_bound;
    use super::*;
    use crate::lludp::error::LLUdpError;

    #[test]
    fn typed_and_schema_routes_are_explicit() {
        for (id, strategy) in [
            (schema::ids::USE_CIRCUIT_CODE, AllocationStrategy::NoAlloc),
            (schema::ids::COMPLETE_AGENT_MOVEMENT, AllocationStrategy::NoAlloc),
            (schema::ids::REGION_HANDSHAKE, AllocationStrategy::Specialized),
            (schema::ids::AGENT_MOVEMENT_COMPLETE, AllocationStrategy::SchemaDerived),
            (schema::ids::PACKET_ACK, AllocationStrategy::SchemaDerived),
            (schema::ids::OBJECT_UPDATE, AllocationStrategy::Specialized),
        ] {
            assert_eq!(Message::typed_allocation_strategy(id), Some(strategy));
            assert_eq!(Message::allocation_strategy(id), strategy);
        }

        let schema_only_id = schema::find_by_name("NeighborList").expect("schema-only message exists").id;
        assert_eq!(Message::typed_allocation_strategy(schema_only_id), None);
        assert_eq!(Message::allocation_strategy(schema_only_id), AllocationStrategy::SchemaDerived);

        let unknown_id = MessageId::high(u32::from(u16::MAX));
        assert!(schema::find_by_id(unknown_id).is_none());
        assert_eq!(Message::typed_allocation_strategy(unknown_id), None);
        assert_eq!(Message::allocation_strategy(unknown_id), AllocationStrategy::Opaque);
        assert_eq!(fallback_allocation_bound(unknown_id, &[]), Ok((AllocationStrategy::Opaque, 0)));
    }

    #[test]
    fn every_specialized_message_has_a_preflight_route() {
        for metadata in schema::MESSAGES.iter() {
            if Message::allocation_strategy(metadata.id) == AllocationStrategy::Specialized {
                assert_ne!(
                    Message::allocation_bound(metadata.id, &[]),
                    Err(LLUdpError::InvalidMessageId),
                    "{} must have an explicit specialized allocation bound",
                    metadata.name
                );
            }
        }
    }
}
