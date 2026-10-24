use crate::{LLUdpError, lludp::codec::Reader};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KillObject {
    pub object_data: Vec<ObjectData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectData {
    pub local_id: u32,
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<KillObject, LLUdpError> {
    let count = reader.terminal_variable_count()?;
    let mut object_data = Vec::with_capacity(count);

    for _ in 0..count {
        object_data.push(ObjectData { local_id: reader.u32()? });
    }

    Ok(KillObject { object_data })
}
