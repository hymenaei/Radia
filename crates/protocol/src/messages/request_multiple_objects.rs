use base::UUID;

use crate::{codec::Reader, error::LLUdpError};

/// Viewer request for full object data after a cache miss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestMultipleObjects {
    pub agent_data: RequestAgentData,
    pub objects: Vec<ObjectRequest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestAgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}

/// One cache miss request. Unknown wire values remain representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectRequest {
    pub cache_miss_type: CacheMissType,
    pub local_id: u32,
}

/// The reason the viewer requests object data after a cache lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheMissType {
    /// Request the complete object data. Wire value `0`.
    Full,
    /// Request a refresh because the cached CRC differs. Wire value `1`.
    CrcMismatch,
    /// Preserve a cache-miss value this version does not interpret.
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
