use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
    messages::object::{
        ExtraParameter, ObjectMotion, ObjectSound, ObjectText, RegionData, add_preflight_capacity, decode_extra_parameters,
        encode_extra_parameters, preflight_capacity, rotation_from_vector3, skip_extra_parameters,
    },
    types::ObjectShape,
};
use math::{Vec3, Vec4};

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
        let packed_motion = reader.variable1()?;
        let DecodedMotion { motion, avatar_collision_plane } = decode_motion(packed_motion)?;
        let parent_local_id = reader.u32()?;
        let update_flags = reader.u32()?;

        let shape = ObjectShape::decode(reader)?;
        let texture_entry = reader.variable2()?.to_vec();
        let texture_anim = reader.variable1()?.to_vec();
        let name_value = reader.variable2()?.to_vec();
        let data = reader.variable2()?.to_vec();
        let text = ObjectText { text: reader.variable1_string()?, color: reader.take(4)?.try_into().expect("four bytes requested") };
        let media_url = reader.variable1_string()?;
        let particle_system = reader.variable1()?.to_vec();
        let extra_param_bytes = reader.variable1()?;
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
        let motion = reader.variable1()?;
        if !matches!(motion.len(), 60 | 76 | 124 | 140) {
            return Err(LLUdpError::InvalidMessage);
        }
        reader.take(8)?;
        ObjectShape::decode(&mut reader)?;
        reader.variable2()?;
        reader.variable1()?;
        reader.variable2()?;
        reader.variable2()?;
        reader.variable1()?;
        reader.take(4)?;
        reader.variable1()?;
        reader.variable1()?;

        let extra_parameter_bytes = reader.variable1()?;
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

pub(crate) fn encode_fields(object: &ObjectData, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    writer.u32(object.local_id);
    writer.u8(object.state);
    writer.uuid(object.id);
    writer.u32(object.crc);
    writer.u8(object.pcode);
    writer.u8(object.material);
    writer.u8(object.click_action);
    writer.finite_vector3(object.scale)?;
    encode_motion(object, writer)?;
    writer.u32(object.parent_local_id);
    writer.u32(object.update_flags);
    object.shape.encode(writer);
    writer.variable2(&object.texture_entry)?;
    writer.variable1(&object.texture_anim)?;
    writer.variable2(&object.name_value)?;
    writer.variable2(&object.data)?;
    writer.variable1_string(&object.text.text)?;
    writer.bytes(&object.text.color);
    writer.variable1_string(&object.media_url)?;
    writer.variable1(&object.particle_system)?;
    let mut extra_parameters = Vec::new();
    let mut extra_writer = Writer::with_limit(&mut extra_parameters, usize::MAX);
    encode_extra_parameters(&object.extra_parameters, &mut extra_writer)?;
    extra_writer.finish()?;
    writer.variable1(&extra_parameters)?;
    writer.uuid(object.sound.sound_id);
    writer.uuid(object.owner_id);
    write_finite_f32(writer, object.sound.gain)?;
    writer.u8(object.sound.flags);
    write_finite_f32(writer, object.sound.radius)?;
    writer.u8(object.joint_type);
    writer.finite_vector3(object.joint_pivot)?;
    writer.finite_vector3(object.joint_axis_or_anchor)?;
    Ok(())
}

fn encode_motion(object: &ObjectData, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    let has_collision_plane = object.collision_plane.is_some();
    let has_plane = usize::from(has_collision_plane) * 16;
    let len = 60 + has_plane;
    writer.u8(u8::try_from(len).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    if let Some(plane) = object.collision_plane {
        writer.finite_vector4(plane)?;
    }
    writer.finite_vector3(object.motion.position)?;
    writer.finite_vector3(object.motion.velocity)?;
    writer.finite_vector3(object.motion.acceleration)?;
    writer.quaternion_xyz(object.motion.rotation)?;
    writer.finite_vector3(object.motion.angular_velocity)?;
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
    let avatar_collision_plane = if has_collision_plane { Some(reader.vector4()?) } else { None };
    let position = reader.vector3()?;
    let velocity = reader.vector3()?;
    let acceleration = reader.vector3()?;
    let rotation = rotation_from_vector3(reader.vector3()?);
    let angular_velocity = reader.vector3()?;
    reader.take(ignored_extension_len)?;
    Ok(DecodedMotion { motion: ObjectMotion { position, velocity, acceleration, rotation, angular_velocity }, avatar_collision_plane })
}
