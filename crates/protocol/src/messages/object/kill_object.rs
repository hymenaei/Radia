use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::KILL_OBJECT,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KillObject {
    pub object_data: Vec<ObjectData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectData {
    pub local_id: u32,
}

impl TypedMessage for KillObject {
    const ID: MessageId = KILL_OBJECT;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        self::decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.u8(u8::try_from(self.object_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for object in &self.object_data {
            writer.u32(object.local_id);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.object_data.capacity(), std::mem::size_of::<ObjectData>())
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<KillObject, LLUdpError> {
    let count = reader.terminal_variable_count()?;
    let mut object_data = Vec::with_capacity(count);

    for _ in 0..count {
        object_data.push(ObjectData { local_id: reader.u32()? });
    }

    Ok(KillObject { object_data })
}
