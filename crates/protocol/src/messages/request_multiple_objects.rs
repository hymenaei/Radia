use uuid::Uuid;

use crate::{LLUdpError, lludp::codec::Reader};

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

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<RequestMultipleObjects, LLUdpError> {
    let agent_data = RequestAgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? };
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        objects.push(ObjectRequest { cache_miss_type: CacheMissType::from_wire(reader.u8()?), local_id: reader.u32()? });
    }
    Ok(RequestMultipleObjects { agent_data, objects })
}
