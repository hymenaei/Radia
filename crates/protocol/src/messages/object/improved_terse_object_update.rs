use std::mem::size_of;

use math::{Vec3, Vec4};

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    messages::object::{ObjectMotion, RegionData, decode_quantized, encode_quantized, rotation_from_u16},
    schema::ids::IMPROVED_TERSE_OBJECT_UPDATE,
    types::{TextureEntry, decode_texture_entry as decode_texture_entry_payload},
};

pub const MAX_TEXTURE_ENTRY_FIELD_BYTES: usize = 1024;
pub const MAX_TEXTURE_ENTRY_BYTES: usize = 1020;

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

impl TypedMessage for ImprovedTerseObjectUpdate {
    const ID: MessageId = IMPROVED_TERSE_OBJECT_UPDATE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::Specialized;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        self::decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        self.region_data.encode(writer);
        let count = u8::try_from(self.objects.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
        writer.u8(count);
        for block in &self.objects {
            match block {
                ObjectBlock::Decoded(object) => {
                    if object.avatar != object.collision_plane.is_some() {
                        return Err(LLUdpError::InvalidMessage);
                    }
                    writer.u8(if object.collision_plane.is_some() { 60 } else { 44 });
                    writer.u32(object.local_id);
                    writer.u8(object.state);
                    writer.u8(u8::from(object.avatar));
                    if let Some(plane) = object.collision_plane {
                        writer.finite_vector4(plane)?;
                    }
                    writer.finite_vector3(object.motion.position)?;
                    write_quantized_vector3(writer, object.motion.velocity, -128.0, 128.0)?;
                    write_quantized_vector3(writer, object.motion.acceleration, -64.0, 64.0)?;
                    for value in [object.motion.rotation.x, object.motion.rotation.y, object.motion.rotation.z, object.motion.rotation.w] {
                        writer.u16(encode_quantized(value, -1.0, 1.0)?);
                    }
                    write_quantized_vector3(writer, object.motion.angular_velocity, -64.0, 64.0)?;
                    let texture_len = u16::try_from(object.texture_entry.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                    writer.u16(texture_len);
                    writer.bytes(&object.texture_entry);
                }
                ObjectBlock::Malformed { packed_data, texture_entry } => {
                    writer.u8(u8::try_from(packed_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
                    writer.bytes(packed_data);
                    writer.u16(u16::try_from(texture_entry.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
                    writer.bytes(texture_entry);
                }
            }
        }
        Ok(())
    }

    fn preflight_allocation_bound(payload: &[u8]) -> Result<usize, LLUdpError> {
        crate::lludp::message::allocation::with_payload(payload, preflight_allocation_bytes(payload)?)
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(self.objects.capacity(), std::mem::size_of::<ObjectBlock>());
        for block in &self.objects {
            match block {
                ObjectBlock::Decoded(object) => {
                    crate::lludp::message::allocation::add_capacity(&mut bytes, object.texture_entry.capacity(), std::mem::size_of::<u8>())
                }
                ObjectBlock::Malformed { packed_data, texture_entry } => {
                    crate::lludp::message::allocation::add_capacity(&mut bytes, packed_data.capacity(), std::mem::size_of::<u8>());
                    crate::lludp::message::allocation::add_capacity(&mut bytes, texture_entry.capacity(), std::mem::size_of::<u8>());
                }
            }
        }
        bytes
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ImprovedTerseObjectUpdate, LLUdpError> {
    let region_data = RegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);

    for _ in 0..count {
        let packed_data = reader.variable1()?;
        let texture_entry = reader.variable2()?;
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
        let packed_data = reader.variable1()?;
        let texture_entry = reader.variable2()?;
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
    let collision_plane = if avatar { Some(data.vector4()?) } else { None };
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

fn write_quantized_vector3(writer: &mut Writer<'_>, value: Vec3, lower: f32, upper: f32) -> Result<(), LLUdpError> {
    writer.u16(encode_quantized(value.x, lower, upper)?);
    writer.u16(encode_quantized(value.y, lower, upper)?);
    writer.u16(encode_quantized(value.z, lower, upper)?);
    Ok(())
}
