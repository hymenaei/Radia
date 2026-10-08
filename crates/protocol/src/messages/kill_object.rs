use crate::{codec::Reader, error::UdpError};

/// Simulator removal notification containing local object IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KillObject {
    pub local_ids: Vec<u32>,
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<KillObject, UdpError> {
    let count = reader.terminal_variable_count()?;
    let mut local_ids = Vec::with_capacity(count);
    for _ in 0..count {
        local_ids.push(reader.u32()?);
    }
    Ok(KillObject { local_ids })
}
