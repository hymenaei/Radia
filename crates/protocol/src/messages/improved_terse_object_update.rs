use std::mem::size_of;

use crate::{
    LLUdpError,
    lludp::codec::Reader,
    messages::common::{ObjectMotion, RegionData, decode_quantized, read_variable1, read_variable2, read_vector4, rotation_from_u16},
    types::{TextureEntry, decode_texture_entry as decode_texture_entry_payload},
};
use math::{Vec3, Vec4};

pub const MAX_TEXTURE_ENTRY_FIELD_BYTES: usize = 1024;
pub const MAX_TEXTURE_ENTRY_BYTES: usize = 1020;

pub fn decode_texture_entry(bytes: &[u8]) -> Result<Option<TextureEntry>, LLUdpError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > MAX_TEXTURE_ENTRY_FIELD_BYTES {
        return Err(LLUdpError::InvalidMessage);
    }
    let length_bytes: [u8; 4] = bytes.get(..4).ok_or(LLUdpError::InvalidMessage)?.try_into().expect("four bytes requested");
    let length = i32::from_le_bytes(length_bytes);
    if length < 0 {
        return Err(LLUdpError::InvalidMessage);
    }
    let length = usize::try_from(length).map_err(|_| LLUdpError::InvalidMessage)?;
    if length > MAX_TEXTURE_ENTRY_BYTES {
        return Err(LLUdpError::InvalidMessage);
    }
    let end = 4_usize.checked_add(length).ok_or(LLUdpError::InvalidMessage)?;
    let payload = bytes.get(4..end).ok_or(LLUdpError::InvalidMessage)?;
    decode_texture_entry_payload(payload).map_err(|_| LLUdpError::InvalidMessage)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImprovedTerseObjectUpdate {
    pub region_data: RegionData,
    pub objects: Vec<ObjectBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ObjectBlock {
    Decoded(ObjectData),
    Malformed { packed_data: Vec<u8>, texture_entry: Vec<u8> },
}

impl ObjectBlock {
    pub const fn as_decoded(&self) -> Option<&ObjectData> {
        match self {
            Self::Decoded(object) => Some(object),
            Self::Malformed { .. } => None,
        }
    }

    pub fn as_decoded_mut(&mut self) -> Option<&mut ObjectData> {
        match self {
            Self::Decoded(object) => Some(object),
            Self::Malformed { .. } => None,
        }
    }
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
        match decode_object_data(packed_data, texture_entry) {
            Ok(object) => objects.push(ObjectBlock::Decoded(object)),
            Err(_) => objects.push(ObjectBlock::Malformed { packed_data: packed_data.to_vec(), texture_entry: texture_entry.to_vec() }),
        }
    }

    Ok(ImprovedTerseObjectUpdate { region_data, objects })
}

pub(crate) fn preflight_allocation_bytes(payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut reader = Reader::new(payload);
    RegionData::decode(&mut reader)?;
    let count = reader.terminal_variable_count()?;
    let mut allocation_bytes = count.checked_mul(size_of::<ObjectBlock>()).ok_or(LLUdpError::AllocationBoundOverflow)?;

    for _ in 0..count {
        let packed_data = read_variable1(&mut reader)?;
        let texture_entry = read_variable2(&mut reader)?;
        let malformed = packed_data.get(5).is_none_or(|avatar| packed_data.len() < if *avatar == 0 { 44 } else { 60 });
        let nested_bytes = if malformed { packed_data.len().saturating_add(texture_entry.len()) } else { texture_entry.len() };
        allocation_bytes = allocation_bytes.checked_add(nested_bytes).ok_or(LLUdpError::AllocationBoundOverflow)?;
    }

    Ok(allocation_bytes)
}

fn decode_object_data(packed_data: &[u8], texture_entry: &[u8]) -> Result<ObjectData, LLUdpError> {
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
    Ok(ObjectData {
        local_id,
        state,
        avatar,
        collision_plane,
        motion: ObjectMotion { position, velocity, acceleration, rotation, angular_velocity },
        texture_entry: texture_entry.to_vec(),
    })
}

fn decode_vector3(reader: &mut Reader<'_>, lower: f32, upper: f32) -> Result<Vec3, LLUdpError> {
    let value = Vec3::new(
        decode_quantized(reader.u16()?, lower, upper),
        decode_quantized(reader.u16()?, lower, upper),
        decode_quantized(reader.u16()?, lower, upper),
    );
    if value.x.is_finite() && value.y.is_finite() && value.z.is_finite() { Ok(value) } else { Err(LLUdpError::InvalidMessage) }
}
