use base::UUID;

use crate::{
    Vector3,
    codec::Reader,
    error::UdpError,
    messages::object_data::{
        ObjectMotion, ObjectRegionData, ObjectRotation, preserve_fixed, preserve_variable1, preserve_variable2, read_finite_array4,
        read_finite_vector3, read_variable1,
    },
};

/// A full simulator object update.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectUpdate {
    pub region_data: ObjectRegionData,
    /// Full object records in packet order.
    pub objects: Vec<FullObjectUpdate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FullObjectUpdate {
    /// Region-local simulator ID. The simulator can reuse it after an object leaves.
    pub local_id: u32,
    pub state: u8,
    pub id: UUID,
    pub crc: u32,
    /// Raw object-type protocol code.
    pub pcode: u8,
    pub material: u8,
    pub click_action: u8,
    /// Object dimensions in meters along each axis.
    pub scale: Vector3,
    /// Position and motion values. Full-update encoding writes only rotation x, y, and z. It ignores w.
    /// Decoding rebuilds w as `sqrt(1 - x² - y² - z²)` when the result is nonnegative, or zero otherwise.
    pub motion: ObjectMotion,
    pub avatar_collision_plane: Option<[f32; 4]>,
    /// Parent's region-local ID, or zero when the object has no parent.
    pub parent_local_id: u32,
    pub update_flags: u32,
    /// Uninterpreted full-update fields after the flags, retained in wire order for re-encoding.
    pub opaque_data: Vec<u8>,
    /// The 64-byte motion extension in extended layouts, retained in wire order.
    /// This is empty in the 60-byte and 76-byte layouts.
    pub opaque_motion_data: Vec<u8>,
}

struct DecodedMotion {
    motion: ObjectMotion,
    avatar_collision_plane: Option<[f32; 4]>,
    opaque_motion_data: Vec<u8>,
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectUpdate, UdpError> {
    let region_data = ObjectRegionData::decode(reader)?;
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
        let scale = read_finite_vector3(reader)?;
        let packed_motion = read_variable1(reader)?;
        let DecodedMotion { motion, avatar_collision_plane, opaque_motion_data } = decode_motion(packed_motion)?;
        let parent_local_id = reader.u32()?;
        let update_flags = reader.u32()?;

        let mut opaque_data = Vec::new();
        preserve_fixed(reader, 23, &mut opaque_data)?;
        preserve_variable2(reader, &mut opaque_data)?; // TextureEntry
        preserve_variable1(reader, &mut opaque_data)?; // TextureAnim
        preserve_variable2(reader, &mut opaque_data)?; // NameValue
        preserve_variable2(reader, &mut opaque_data)?; // Data
        preserve_variable1(reader, &mut opaque_data)?; // Text
        preserve_fixed(reader, 4, &mut opaque_data)?; // TextColor
        preserve_variable1(reader, &mut opaque_data)?; // MediaURL
        preserve_variable1(reader, &mut opaque_data)?; // PSBlock
        preserve_variable1(reader, &mut opaque_data)?; // ExtraParams
        preserve_fixed(reader, 41, &mut opaque_data)?; // Sound, OwnerID, Gain, Flags, Radius
        preserve_fixed(reader, 25, &mut opaque_data)?; // JointType and two vectors

        objects.push(FullObjectUpdate {
            local_id,
            state,
            id,
            crc,
            pcode,
            material,
            click_action,
            scale,
            motion,
            avatar_collision_plane,
            parent_local_id,
            update_flags,
            opaque_data,
            opaque_motion_data,
        });
    }

    Ok(ObjectUpdate { region_data, objects })
}

fn decode_motion(bytes: &[u8]) -> Result<DecodedMotion, UdpError> {
    let (has_collision_plane, tail_len) = match bytes.len() {
        60 => (false, 0),
        76 => (true, 0),
        124 => (false, 64),
        140 => (true, 64),
        _ => return Err(UdpError::InvalidMessage),
    };
    let mut reader = Reader::new(bytes);
    let avatar_collision_plane = if has_collision_plane { Some(read_finite_array4(&mut reader)?) } else { None };
    let position = read_finite_vector3(&mut reader)?;
    let velocity = read_finite_vector3(&mut reader)?;
    let acceleration = read_finite_vector3(&mut reader)?;
    let rotation = ObjectRotation::from_vector3(read_finite_vector3(&mut reader)?)?;
    let angular_velocity = read_finite_vector3(&mut reader)?;
    let opaque_motion_data = reader.take(tail_len)?.to_vec();
    Ok(DecodedMotion {
        motion: ObjectMotion { position, velocity, acceleration, rotation, angular_velocity },
        avatar_collision_plane,
        opaque_motion_data,
    })
}
