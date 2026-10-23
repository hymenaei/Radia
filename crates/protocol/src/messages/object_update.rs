use std::mem::size_of;

use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
    messages::common::{
        ExtraParameter, ObjectMotion, ObjectSound, ObjectText, RegionData, decode_extra_parameters, encode_extra_parameters,
        read_variable1, read_variable2, read_vector4, rotation_from_vector3, skip_extra_parameters, write_finite_vector3,
        write_finite_vector4, write_variable1, write_variable2,
    },
    types::{ObjectShape, TextureEntry, decode_texture_entry as decode_texture_entry_payload},
};
use math::{Vec3, Vec4};

pub const MAX_TEXTURE_ENTRY_BYTES: usize = 4095;

pub fn decode_texture_entry(bytes: &[u8]) -> Result<Option<TextureEntry>, LLUdpError> {
    if bytes.len() > MAX_TEXTURE_ENTRY_BYTES {
        return Err(LLUdpError::InvalidMessage);
    }
    decode_texture_entry_payload(bytes).map_err(|_| LLUdpError::InvalidMessage)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectUpdate {
    pub region_data: RegionData,
    pub objects: Vec<ObjectData>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectData {
    pub local_id: u32,
    pub state: u8,
    pub id: Uuid,
    pub crc: u32,
    pub pcode: u8,
    pub material: u8,
    pub click_action: u8,
    pub scale: Vec3,
    pub motion: ObjectMotion,
    pub collision_plane: Option<Vec4>,
    pub parent_local_id: u32,
    pub update_flags: u32,
    pub shape: ObjectShape,
    pub texture_entry: Vec<u8>,
    pub texture_anim: Vec<u8>,
    pub name_value: Vec<u8>,
    pub data: Vec<u8>,
    pub text: ObjectText,
    pub media_url: String,
    pub particle_system: Vec<u8>,
    pub extra_parameters: Vec<ExtraParameter>,
    pub sound: ObjectSound,
    pub owner_id: Uuid,
    pub joint_type: u8,
    pub joint_pivot: Vec3,
    pub joint_axis_or_anchor: Vec3,
}

struct DecodedMotion {
    motion: ObjectMotion,
    avatar_collision_plane: Option<Vec4>,
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectUpdate, LLUdpError> {
    let region_data = RegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);

    for _ in 0..count {
        let local_id = reader.u32()?;
        let state = reader.u8()?;
        let id = reader.uuid()?;
        let crc = reader.u32()?;
        let pcode = reader.u8()?;
        let material = reader.u8()?;
        let click_action = reader.u8()?;
        let scale = reader.vector3()?;
        let packed_motion = read_variable1(reader)?;
        let DecodedMotion { motion, avatar_collision_plane } = decode_motion(packed_motion)?;
        let parent_local_id = reader.u32()?;
        let update_flags = reader.u32()?;

        let shape = ObjectShape::decode(reader)?;
        let texture_entry = read_variable2(reader)?.to_vec();
        let texture_anim = read_variable1(reader)?.to_vec();
        let name_value = read_variable2(reader)?.to_vec();
        let data = read_variable2(reader)?.to_vec();
        let text = ObjectText { text: reader.variable1_string()?, color: reader.take(4)?.try_into().expect("four bytes requested") };
        let media_url = reader.variable1_string()?;
        let particle_system = read_variable1(reader)?.to_vec();
        let extra_param_bytes = read_variable1(reader)?;
        let mut extra_param_reader = Reader::new(extra_param_bytes);
        let extra_parameters = if extra_param_bytes.is_empty() { Vec::new() } else { decode_extra_parameters(&mut extra_param_reader)? };
        if !extra_param_reader.is_empty() {
            return Err(LLUdpError::InvalidMessage);
        }
        let sound_id = reader.uuid()?;
        let owner_id = reader.uuid()?;
        let gain = reader.f32()?;
        let flags = reader.u8()?;
        let radius = reader.f32()?;
        let sound = ObjectSound { sound_id, gain, flags, radius };
        let joint_type = reader.u8()?;
        let joint_pivot = reader.vector3()?;
        let joint_axis_or_anchor = reader.vector3()?;

        objects.push(ObjectData {
            local_id,
            state,
            id,
            crc,
            pcode,
            material,
            click_action,
            scale,
            motion,
            collision_plane: avatar_collision_plane,
            parent_local_id,
            update_flags,
            shape,
            texture_entry,
            texture_anim,
            name_value,
            data,
            text,
            media_url,
            particle_system,
            extra_parameters,
            sound,
            owner_id,
            joint_type,
            joint_pivot,
            joint_axis_or_anchor,
        });
    }

    Ok(ObjectUpdate { region_data, objects })
}

pub(crate) fn preflight_allocation_bytes(payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut reader = Reader::new(payload);
    RegionData::decode(&mut reader)?;
    let count = reader.terminal_variable_count()?;
    let mut allocation_bytes = preflight_capacity::<ObjectData>(count)?;

    for _ in 0..count {
        reader.take(40)?;
        let motion = read_variable1(&mut reader)?;
        if !matches!(motion.len(), 60 | 76 | 124 | 140) {
            return Err(LLUdpError::InvalidMessage);
        }
        reader.take(8)?;
        ObjectShape::decode(&mut reader)?;
        read_variable2(&mut reader)?;
        read_variable1(&mut reader)?;
        read_variable2(&mut reader)?;
        read_variable2(&mut reader)?;
        read_variable1(&mut reader)?;
        reader.take(4)?;
        read_variable1(&mut reader)?;
        read_variable1(&mut reader)?;

        let extra_parameter_bytes = read_variable1(&mut reader)?;
        if !extra_parameter_bytes.is_empty() {
            let mut extra_parameters = Reader::new(extra_parameter_bytes);
            let count = skip_extra_parameters(&mut extra_parameters)?;
            add_preflight_capacity::<ExtraParameter>(&mut allocation_bytes, count)?;
            if !extra_parameters.is_empty() {
                return Err(LLUdpError::InvalidMessage);
            }
        }

        reader.take(66)?;
    }

    Ok(allocation_bytes)
}

fn preflight_capacity<T>(count: usize) -> Result<usize, LLUdpError> {
    count.checked_mul(size_of::<T>()).ok_or(LLUdpError::AllocationBoundOverflow)
}

fn add_preflight_capacity<T>(total: &mut usize, count: usize) -> Result<(), LLUdpError> {
    *total = total.checked_add(preflight_capacity::<T>(count)?).ok_or(LLUdpError::AllocationBoundOverflow)?;
    Ok(())
}

pub(crate) fn encode_fields(object: &ObjectData, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    writer.u32(object.local_id);
    writer.u8(object.state);
    writer.uuid(object.id);
    writer.u32(object.crc);
    writer.u8(object.pcode);
    writer.u8(object.material);
    writer.u8(object.click_action);
    write_finite_vector3(writer, object.scale)?;
    encode_motion(object, writer)?;
    writer.u32(object.parent_local_id);
    writer.u32(object.update_flags);
    object.shape.encode(writer);
    write_variable2(writer, &object.texture_entry)?;
    write_variable1(writer, &object.texture_anim)?;
    write_variable2(writer, &object.name_value)?;
    write_variable2(writer, &object.data)?;
    writer.variable1_string(&object.text.text)?;
    writer.bytes(&object.text.color);
    writer.variable1_string(&object.media_url)?;
    write_variable1(writer, &object.particle_system)?;
    let mut extra_parameters = Vec::new();
    let mut extra_writer = Writer::with_limit(&mut extra_parameters, usize::MAX);
    encode_extra_parameters(&object.extra_parameters, &mut extra_writer)?;
    extra_writer.finish()?;
    write_variable1(writer, &extra_parameters)?;
    writer.uuid(object.sound.sound_id);
    writer.uuid(object.owner_id);
    write_finite_f32(writer, object.sound.gain)?;
    writer.u8(object.sound.flags);
    write_finite_f32(writer, object.sound.radius)?;
    writer.u8(object.joint_type);
    write_finite_vector3(writer, object.joint_pivot)?;
    write_finite_vector3(writer, object.joint_axis_or_anchor)?;
    Ok(())
}

fn encode_motion(object: &ObjectData, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    let has_collision_plane = object.collision_plane.is_some();
    let has_plane = usize::from(has_collision_plane) * 16;
    let len = 60 + has_plane;
    writer.u8(u8::try_from(len).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    if let Some(plane) = object.collision_plane {
        write_finite_vector4(writer, plane)?;
    }
    write_finite_vector3(writer, object.motion.position)?;
    write_finite_vector3(writer, object.motion.velocity)?;
    write_finite_vector3(writer, object.motion.acceleration)?;
    writer.quaternion_xyz(object.motion.rotation)?;
    write_finite_vector3(writer, object.motion.angular_velocity)?;
    Ok(())
}

fn write_finite_f32(writer: &mut Writer<'_>, value: f32) -> Result<(), LLUdpError> {
    if value.is_finite() {
        writer.f32(value);
        Ok(())
    } else {
        Err(LLUdpError::InvalidMessage)
    }
}

fn decode_motion(bytes: &[u8]) -> Result<DecodedMotion, LLUdpError> {
    let (has_collision_plane, ignored_extension_len) = match bytes.len() {
        60 => (false, 0),
        76 => (true, 0),
        124 => (false, 64),
        140 => (true, 64),
        _ => return Err(LLUdpError::InvalidMessage),
    };
    let mut reader = Reader::new(bytes);
    let avatar_collision_plane = if has_collision_plane { Some(read_vector4(&mut reader)?) } else { None };
    let position = reader.vector3()?;
    let velocity = reader.vector3()?;
    let acceleration = reader.vector3()?;
    let rotation = rotation_from_vector3(reader.vector3()?);
    let angular_velocity = reader.vector3()?;
    reader.take(ignored_extension_len)?;
    Ok(DecodedMotion { motion: ObjectMotion { position, velocity, acceleration, rotation, angular_velocity }, avatar_collision_plane })
}
