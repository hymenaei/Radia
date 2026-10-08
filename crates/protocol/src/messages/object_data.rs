use crate::{
    Vector3,
    codec::{Reader, Writer},
    error::UdpError,
};

/// Region context shared by simulator object updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectRegionData {
    /// Global simulator region handle.
    pub region_handle: u64,
    /// Simulator time dilation encoded as an unsigned 16-bit fraction.
    pub time_dilation: u16,
}

/// A complete set of motion values carried by an object update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectMotion {
    /// Position in simulator region coordinates.
    pub position: Vector3,
    /// Linear velocity in meters per second.
    pub velocity: Vector3,
    /// Linear acceleration in meters per second squared.
    pub acceleration: Vector3,
    pub rotation: ObjectRotation,
    /// Angular velocity in radians per second.
    pub angular_velocity: Vector3,
}

/// Object rotation with all four quaternion components.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ObjectRotation {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl ObjectRegionData {
    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, UdpError> {
        Ok(Self { region_handle: reader.u64()?, time_dilation: reader.u16()? })
    }

    pub(crate) fn encode(self, writer: &mut Writer<'_>) {
        writer.u64(self.region_handle);
        writer.u16(self.time_dilation);
    }
}

impl ObjectRotation {
    pub(crate) fn from_vector3(value: Vector3) -> Result<Self, UdpError> {
        validate_vector3(value)?;
        let squared =
            f64::from(value.x) * f64::from(value.x) + f64::from(value.y) * f64::from(value.y) + f64::from(value.z) * f64::from(value.z);
        let w = if squared < 1.0 { (1.0 - squared).sqrt() as f32 } else { 0.0 };
        Ok(Self { x: value.x, y: value.y, z: value.z, w })
    }

    pub(crate) fn from_u16(values: [u16; 4]) -> Self {
        let [x, y, z, w] = values.map(|value| decode_quantized(value, -1.0, 1.0));
        Self { x, y, z, w }
    }
}

pub(crate) fn read_finite_vector3(reader: &mut Reader<'_>) -> Result<Vector3, UdpError> {
    let value = reader.vector3()?;
    validate_vector3(value)?;
    Ok(value)
}

pub(crate) fn validate_vector3(value: Vector3) -> Result<(), UdpError> {
    if value.x.is_finite() && value.y.is_finite() && value.z.is_finite() { Ok(()) } else { Err(UdpError::InvalidMessage) }
}

pub(crate) fn write_finite_vector3(writer: &mut Writer<'_>, value: Vector3) -> Result<(), UdpError> {
    validate_vector3(value)?;
    writer.vector3(value);
    Ok(())
}

pub(crate) fn read_finite_array4(reader: &mut Reader<'_>) -> Result<[f32; 4], UdpError> {
    let values = [reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?];
    if values.iter().all(|value| value.is_finite()) { Ok(values) } else { Err(UdpError::InvalidMessage) }
}

pub(crate) fn write_finite_array4(writer: &mut Writer<'_>, values: [f32; 4]) -> Result<(), UdpError> {
    if values.iter().all(|value| value.is_finite()) {
        for value in values {
            writer.f32(value);
        }
        Ok(())
    } else {
        Err(UdpError::InvalidMessage)
    }
}

pub(crate) fn read_variable1<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], UdpError> {
    let len = usize::from(reader.u8()?);
    reader.take(len)
}

pub(crate) fn read_variable2<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], UdpError> {
    let len = usize::from(reader.u16()?);
    reader.take(len)
}

pub(crate) fn preserve_fixed(reader: &mut Reader<'_>, len: usize, output: &mut Vec<u8>) -> Result<(), UdpError> {
    output.extend_from_slice(reader.take(len)?);
    Ok(())
}

pub(crate) fn preserve_variable1(reader: &mut Reader<'_>, output: &mut Vec<u8>) -> Result<(), UdpError> {
    let prefix = reader.take(1)?;
    output.extend_from_slice(prefix);
    let len = usize::from(prefix[0]);
    preserve_fixed(reader, len, output)
}

pub(crate) fn preserve_variable2(reader: &mut Reader<'_>, output: &mut Vec<u8>) -> Result<(), UdpError> {
    let prefix = reader.take(2)?;
    output.extend_from_slice(prefix);
    let len = usize::from(u16::from_le_bytes([prefix[0], prefix[1]]));
    preserve_fixed(reader, len, output)
}

pub(crate) fn decode_quantized(value: u16, lower: f32, upper: f32) -> f32 {
    let normalized = f32::from(value) * (1.0 / f32::from(u16::MAX));
    let delta = upper - lower;
    let decoded = normalized * delta + lower;
    let max_error = delta * (1.0 / f32::from(u16::MAX));
    if decoded.abs() < max_error { 0.0 } else { decoded }
}

pub(crate) fn encode_quantized(value: f32, lower: f32, upper: f32) -> Result<u16, UdpError> {
    if !value.is_finite() || value < lower || value > upper {
        return Err(UdpError::InvalidMessage);
    }
    let scaled = ((value - lower) / (upper - lower)) * f32::from(u16::MAX);
    Ok(scaled.round().clamp(0.0, f32::from(u16::MAX)) as u16)
}

#[cfg(test)]
mod tests {
    use super::decode_quantized;

    #[test]
    fn quantized_values_within_one_step_of_zero_snap_to_zero() {
        assert_eq!(decode_quantized(32_767, -64.0, 64.0), 0.0);
        assert_eq!(decode_quantized(32_768, -64.0, 64.0), 0.0);
        assert!(decode_quantized(32_766, -64.0, 64.0) < 0.0);
        assert!(decode_quantized(32_769, -64.0, 64.0) > 0.0);
    }
}
