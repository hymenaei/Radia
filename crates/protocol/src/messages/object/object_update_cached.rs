use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    messages::object::RegionData,
    schema::ids::OBJECT_UPDATE_CACHED,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectUpdateCached {
    pub region_data: RegionData,
    pub objects: Vec<CachedObjectReference>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CachedObjectReference {
    pub local_id: u32,
    pub crc: u32,
    pub update_flags: u32,
}

impl TypedMessage for ObjectUpdateCached {
    const ID: MessageId = OBJECT_UPDATE_CACHED;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        self::decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        self.region_data.encode(writer);
        let count = u8::try_from(self.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
        writer.u8(count);
        for object in &self.objects {
            writer.u32(object.local_id);
            writer.u32(object.crc);
            writer.u32(object.update_flags);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.objects.capacity(), std::mem::size_of::<CachedObjectReference>())
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectUpdateCached, LLUdpError> {
    let region_data = RegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        objects.push(CachedObjectReference { local_id: reader.u32()?, crc: reader.u32()?, update_flags: reader.u32()? });
    }
    Ok(ObjectUpdateCached { region_data, objects })
}
