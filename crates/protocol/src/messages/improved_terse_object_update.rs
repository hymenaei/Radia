use std::mem::size_of;

use crate::{
    LLUdpError,
    lludp::codec::Reader,
    messages::common::{ObjectMotion, RegionData, decode_quantized, read_variable1, read_variable2, read_vector4, rotation_from_u16},
};
use math::{Vec3, Vec4};

/// A packet of partial object motion updates.
#[derive(Clone, Debug, PartialEq)]
pub struct ImprovedTerseObjectUpdate {
    pub region_data: RegionData,
    pub objects: Vec<ObjectData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectData {
    pub local_id: u32,
    pub state: u8,
    pub avatar: bool,
    pub collision_plane: Option<Vec4>,
    pub motion: ObjectMotion,
    pub texture_entry: Vec<u8>,
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ImprovedTerseObjectUpdate, LLUdpError> {
    let region_data = RegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);

    for _ in 0..count {
        let packed_data = read_variable1(reader)?;
        let texture_entry = read_variable2(reader)?;
        let mut data = Reader::new(packed_data);
        let local_id = data.u32()?;
        let state = data.u8()?;
        let avatar = data.u8()? != 0;
        let collision_plane = if avatar { Some(read_vector4(&mut data)?) } else { None };
        let position = data.vector3()?;
        let velocity = decode_vector3(&mut data, -128.0, 128.0)?;
        let acceleration = decode_vector3(&mut data, -64.0, 64.0)?;
        let rotation = rotation_from_u16([data.u16()?, data.u16()?, data.u16()?, data.u16()?]);
        let angular_velocity = decode_vector3(&mut data, -64.0, 64.0)?;
        if !data.remaining().is_empty() {
            return Err(LLUdpError::InvalidMessage);
        }
        objects.push(ObjectData {
            local_id,
            state,
            avatar,
            collision_plane,
            motion: ObjectMotion { position, velocity, acceleration, rotation, angular_velocity },
            texture_entry: texture_entry.to_vec(),
        });
    }

    Ok(ImprovedTerseObjectUpdate { region_data, objects })
}

pub(crate) fn preflight_allocation_bytes(payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut reader = Reader::new(payload);
    RegionData::decode(&mut reader)?;
    let count = reader.terminal_variable_count()?;
    let allocation_bytes = count.checked_mul(size_of::<ObjectData>()).ok_or(LLUdpError::AllocationBoundOverflow)?;

    for _ in 0..count {
        let packed_data = read_variable1(&mut reader)?;
        read_variable2(&mut reader)?;
        let mut data = Reader::new(packed_data);
        data.take(4 + 1)?;
        let avatar = data.u8()? != 0;
        if avatar {
            data.take(16)?;
        }
        data.take(12 + 6 + 6 + 8 + 6)?;
        if !data.is_empty() {
            return Err(LLUdpError::InvalidMessage);
        }
    }

    Ok(allocation_bytes)
}

fn decode_vector3(reader: &mut Reader<'_>, lower: f32, upper: f32) -> Result<Vec3, LLUdpError> {
    let value = Vec3::new(
        decode_quantized(reader.u16()?, lower, upper),
        decode_quantized(reader.u16()?, lower, upper),
        decode_quantized(reader.u16()?, lower, upper),
    );
    if value.x.is_finite() && value.y.is_finite() && value.z.is_finite() { Ok(value) } else { Err(LLUdpError::InvalidMessage) }
}
