use base::UUID;

use crate::{
    Vector3,
    codec::Reader,
    error::UdpError,
    messages::object_data::{ObjectRegionData, ObjectRotation, read_finite_vector3, read_variable2},
};

/// A compressed full object update.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectUpdateCompressed {
    pub region_data: ObjectRegionData,
    /// Compressed object records in packet order.
    pub objects: Vec<CompressedObjectUpdate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompressedObjectUpdate {
    pub id: UUID,
    /// Region-local simulator ID. The simulator can reuse it after an object leaves.
    pub local_id: u32,
    /// Raw object-type protocol code.
    pub pcode: u8,
    pub state: u8,
    pub crc: u32,
    pub material: u8,
    pub click_action: u8,
    /// Object dimensions in meters along each axis.
    pub scale: Vector3,
    pub position: Vector3,
    /// Rotation. Compressed encoding writes only x, y, and z. It ignores w.
    /// Decoding rebuilds w as `sqrt(1 - x² - y² - z²)` when the result is nonnegative, or zero otherwise.
    pub rotation: ObjectRotation,
    /// Raw special flags. Unknown bits are retained.
    /// The encoder derives bits `0x80` and `0x20` from the angular-velocity and parent fields.
    pub special_flags: u32,
    pub owner_id: UUID,
    /// Angular velocity when special flag `0x80` is set.
    pub angular_velocity: Option<Vector3>,
    /// Parent's region-local ID when special flag `0x20` is set.
    pub parent_local_id: Option<u32>,
    pub update_flags: u32,
    /// Packed bytes after the decoded fields, retained in wire order for re-encoding.
    pub opaque_data: Vec<u8>,
}

const SPECIAL_HAS_ANGULAR_VELOCITY: u32 = 0x80;
const SPECIAL_HAS_PARENT: u32 = 0x20;

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectUpdateCompressed, UdpError> {
    let region_data = ObjectRegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);

    for _ in 0..count {
        let update_flags = reader.u32()?;
        let packed_data = read_variable2(reader)?;
        let mut data = Reader::new(packed_data);
        let id = data.uuid()?;
        let local_id = data.u32()?;
        let pcode = data.u8()?;
        let state = data.u8()?;
        let crc = data.u32()?;
        let material = data.u8()?;
        let click_action = data.u8()?;
        let scale = read_finite_vector3(&mut data)?;
        let position = read_finite_vector3(&mut data)?;
        let rotation = ObjectRotation::from_vector3(read_finite_vector3(&mut data)?)?;
        let special_flags = data.u32()?;
        let owner_id = data.uuid()?;
        let angular_velocity = if special_flags & SPECIAL_HAS_ANGULAR_VELOCITY != 0 { Some(read_finite_vector3(&mut data)?) } else { None };
        let parent_local_id = if special_flags & SPECIAL_HAS_PARENT != 0 { Some(data.u32()?) } else { None };
        let opaque_data = data.remaining().to_vec();

        objects.push(CompressedObjectUpdate {
            id,
            local_id,
            pcode,
            state,
            crc,
            material,
            click_action,
            scale,
            position,
            rotation,
            special_flags,
            owner_id,
            angular_velocity,
            parent_local_id,
            update_flags,
            opaque_data,
        });
    }

    Ok(ObjectUpdateCompressed { region_data, objects })
}
