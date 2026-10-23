use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REQUEST_MULTIPLE_OBJECTS,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestMultipleObjects {
    pub agent_data: RequestAgentData,
    pub objects: Vec<ObjectRequest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestAgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectRequest {
    pub cache_miss_type: CacheMissType,
    pub local_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheMissType {
    Full,
    CrcMismatch,
    Unknown(u8),
}

impl CacheMissType {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::Full,
            1 => Self::CrcMismatch,
            value => Self::Unknown(value),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Full => 0,
            Self::CrcMismatch => 1,
            Self::Unknown(value) => value,
        }
    }
}

impl TypedMessage for RequestMultipleObjects {
    const ID: MessageId = REQUEST_MULTIPLE_OBJECTS;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        self::decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.u8(u8::try_from(self.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for object in &self.objects {
            writer.u8(object.cache_miss_type.to_wire());
            writer.u32(object.local_id);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.objects.capacity(), std::mem::size_of::<ObjectRequest>())
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<RequestMultipleObjects, LLUdpError> {
    let agent_data = RequestAgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? };
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        objects.push(ObjectRequest { cache_miss_type: CacheMissType::from_wire(reader.u8()?), local_id: reader.u32()? });
    }
    Ok(RequestMultipleObjects { agent_data, objects })
}
