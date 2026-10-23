use crate::{LLUdpError, lludp::codec::Reader, messages::object::RegionData};

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

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectUpdateCached, LLUdpError> {
    let region_data = RegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        objects.push(CachedObjectReference { local_id: reader.u32()?, crc: reader.u32()?, update_flags: reader.u32()? });
    }
    Ok(ObjectUpdateCached { region_data, objects })
}
