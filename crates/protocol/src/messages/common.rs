use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
};
use math::{Quat, Vec3, Vec4};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PingId {
    pub ping_id: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionData {
    pub region_handle: u64,
    pub time_dilation: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectMotion {
    pub position: Vec3,
    pub velocity: Vec3,
    pub acceleration: Vec3,
    pub rotation: Quat,
    pub angular_velocity: Vec3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectText {
    pub text: String,
    pub color: [u8; 4],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraParameter {
    pub parameter_type: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectSound {
    pub sound_id: Uuid,
    pub gain: f32,
    pub flags: u8,
    pub radius: f32,
}

impl RegionData {
    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { region_handle: reader.u64()?, time_dilation: reader.u16()? })
    }

    pub(crate) fn encode(self, writer: &mut Writer<'_>) {
        writer.u64(self.region_handle);
        writer.u16(self.time_dilation);
    }
}

pub(crate) fn rotation_from_vector3(value: Vec3) -> Quat {
    let remainder = 1.0 - value.length_squared();
    let w = if remainder > 0.0 { remainder.sqrt() } else { 0.0 };
    Quat::from_xyzw(value.x, value.y, value.z, w)
}

pub(crate) fn rotation_from_u16(values: [u16; 4]) -> Quat {
    let [x, y, z, w] = values.map(|value| decode_quantized(value, -1.0, 1.0));
    Quat::from_xyzw(x, y, z, w)
}

pub(crate) fn validate_vector3(value: Vec3) -> Result<(), LLUdpError> {
    if value.x.is_finite() && value.y.is_finite() && value.z.is_finite() { Ok(()) } else { Err(LLUdpError::InvalidMessage) }
}

pub(crate) fn write_finite_vector3(writer: &mut Writer<'_>, value: Vec3) -> Result<(), LLUdpError> {
    validate_vector3(value)?;
    writer.vector3(value);
    Ok(())
}

pub(crate) fn read_vector4(reader: &mut Reader<'_>) -> Result<Vec4, LLUdpError> {
    Ok(Vec4::new(reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?))
}

pub(crate) fn write_finite_vector4(writer: &mut Writer<'_>, value: Vec4) -> Result<(), LLUdpError> {
    if value.is_finite() {
        writer.f32(value.x);
        writer.f32(value.y);
        writer.f32(value.z);
        writer.f32(value.w);
        Ok(())
    } else {
        Err(LLUdpError::InvalidMessage)
    }
}

pub(crate) fn read_variable1<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], LLUdpError> {
    let len = usize::from(reader.u8()?);
    reader.take(len)
}

pub(crate) fn read_variable2<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], LLUdpError> {
    let len = usize::from(reader.u16()?);
    reader.take(len)
}

pub(crate) fn decode_extra_parameters(reader: &mut Reader<'_>) -> Result<Vec<ExtraParameter>, LLUdpError> {
    let count = usize::from(reader.u8()?);
    let mut parameters = Vec::with_capacity(count);
    for _ in 0..count {
        let (parameter_type, len) = read_extra_parameter_header(reader)?;
        parameters.push(ExtraParameter { parameter_type, data: reader.take(len)?.to_vec() });
    }
    Ok(parameters)
}

pub(crate) fn skip_extra_parameters(reader: &mut Reader<'_>) -> Result<usize, LLUdpError> {
    let count = usize::from(reader.u8()?);
    for _ in 0..count {
        let (_, len) = read_extra_parameter_header(reader)?;
        reader.take(len)?;
    }
    Ok(count)
}

fn read_extra_parameter_header(reader: &mut Reader<'_>) -> Result<(u16, usize), LLUdpError> {
    let parameter_type = reader.u16()?;
    let len = usize::try_from(reader.i32()?).map_err(|_| LLUdpError::InvalidMessage)?;
    Ok((parameter_type, len))
}

pub(crate) fn encode_extra_parameters(parameters: &[ExtraParameter], writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    writer.u8(u8::try_from(parameters.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    for parameter in parameters {
        writer.u16(parameter.parameter_type);
        writer.i32(i32::try_from(parameter.data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        writer.bytes(&parameter.data);
    }
    Ok(())
}

pub(crate) fn write_variable1(writer: &mut Writer<'_>, value: &[u8]) -> Result<(), LLUdpError> {
    writer.u8(u8::try_from(value.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    writer.bytes(value);
    Ok(())
}

pub(crate) fn write_variable2(writer: &mut Writer<'_>, value: &[u8]) -> Result<(), LLUdpError> {
    writer.u16(u16::try_from(value.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    writer.bytes(value);
    Ok(())
}

pub(crate) fn decode_quantized(value: u16, lower: f32, upper: f32) -> f32 {
    let normalized = f32::from(value) * (1.0 / f32::from(u16::MAX));
    let delta = upper - lower;
    let decoded = normalized * delta + lower;
    let max_error = delta * (1.0 / f32::from(u16::MAX));
    if decoded.abs() < max_error { 0.0 } else { decoded }
}

pub(crate) fn encode_quantized(value: f32, lower: f32, upper: f32) -> Result<u16, LLUdpError> {
    if !value.is_finite() || value < lower || value > upper {
        return Err(LLUdpError::InvalidMessage);
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
