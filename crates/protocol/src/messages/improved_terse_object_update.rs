use crate::{
    Vector3,
    codec::Reader,
    error::UdpError,
    messages::object_data::{
        ObjectMotion, ObjectRegionData, ObjectRotation, decode_quantized, read_finite_array4, read_finite_vector3, read_variable1,
        read_variable2,
    },
};

/// A packet of partial object motion updates.
#[derive(Clone, Debug, PartialEq)]
pub struct ImprovedTerseObjectUpdate {
    pub region_data: ObjectRegionData,
    /// Partial object records in packet order.
    pub objects: Vec<TerseObjectUpdate>,
}

/// Motion values for a local object ID. This message does not carry a global ID or scale.
#[derive(Clone, Debug, PartialEq)]
pub struct TerseObjectUpdate {
    /// Region-local simulator ID. The message does not include the object's UUID.
    pub local_id: u32,
    pub state: u8,
    /// Raw presence byte. Any nonzero value means a collision plane follows.
    /// Keep it consistent with `avatar_collision_plane` when encoding.
    pub agent_data_flag: u8,
    /// Avatar collision plane when `agent_data_flag` is nonzero.
    pub avatar_collision_plane: Option<[f32; 4]>,
    /// Partial motion values. Position uses three floats.
    /// Velocity uses 16-bit components in `[-128, 128]`; acceleration and angular velocity use `[-64, 64]`.
    /// The wire encodes all four rotation components as 16-bit values in `[-1, 1]`.
    /// Encoding rejects non-finite or out-of-range values.
    pub motion: ObjectMotion,
    /// Nonempty texture entry bytes. An empty wire entry decodes to `None`.
    pub texture_entry: Option<Vec<u8>>,
    /// Packed bytes after the motion fields and before the texture entry, retained in wire order for re-encoding.
    pub opaque_data: Vec<u8>,
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ImprovedTerseObjectUpdate, UdpError> {
    let region_data = ObjectRegionData::decode(reader)?;
    let count = reader.terminal_variable_count()?;
    let mut objects = Vec::with_capacity(count);

    for _ in 0..count {
        let packed_data = read_variable1(reader)?;
        let texture_entry = read_variable2(reader)?;
        let mut data = Reader::new(packed_data);
        let local_id = data.u32()?;
        let state = data.u8()?;
        let agent_data_flag = data.u8()?;
        let avatar_collision_plane = if agent_data_flag != 0 { Some(read_finite_array4(&mut data)?) } else { None };
        let position = read_finite_vector3(&mut data)?;
        let velocity = decode_vector3(&mut data, -128.0, 128.0)?;
        let acceleration = decode_vector3(&mut data, -64.0, 64.0)?;
        let rotation = ObjectRotation::from_u16([data.u16()?, data.u16()?, data.u16()?, data.u16()?]);
        let angular_velocity = decode_vector3(&mut data, -64.0, 64.0)?;
        let opaque_data = data.remaining().to_vec();
        objects.push(TerseObjectUpdate {
            local_id,
            state,
            agent_data_flag,
            avatar_collision_plane,
            motion: ObjectMotion { position, velocity, acceleration, rotation, angular_velocity },
            texture_entry: (!texture_entry.is_empty()).then(|| texture_entry.to_vec()),
            opaque_data,
        });
    }

    Ok(ImprovedTerseObjectUpdate { region_data, objects })
}

fn decode_vector3(reader: &mut Reader<'_>, lower: f32, upper: f32) -> Result<Vector3, UdpError> {
    let value = Vector3 {
        x: decode_quantized(reader.u16()?, lower, upper),
        y: decode_quantized(reader.u16()?, lower, upper),
        z: decode_quantized(reader.u16()?, lower, upper),
    };
    if value.x.is_finite() && value.y.is_finite() && value.z.is_finite() { Ok(value) } else { Err(UdpError::InvalidMessage) }
}
