use std::str;

use math::{DVec3, Quat, Vec3, Vec4};
use uuid::Uuid;

use super::{error::LLUdpError, types::quaternion_from_xyz};

pub(crate) struct Writer<'a> {
    output: &'a mut Vec<u8>,
    limit: usize,
    overflow_at: Option<usize>,
}

impl<'a> Writer<'a> {
    #[cfg(test)]
    pub(crate) fn new(output: &'a mut Vec<u8>) -> Self {
        Self::with_limit(output, usize::MAX)
    }

    pub(crate) fn with_limit(output: &'a mut Vec<u8>, limit: usize) -> Self {
        let overflow_at = (output.len() > limit).then_some(limit);
        Self { output, limit, overflow_at }
    }

    pub(crate) fn finish(self) -> Result<(), LLUdpError> {
        if self.overflow_at.is_some() { Err(LLUdpError::DatagramTooLarge) } else { Ok(()) }
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        let length = bytes.len().min(self.limit.saturating_sub(self.output.len()));
        self.output.extend_from_slice(&bytes[..length]);
        if length < bytes.len() {
            self.overflow_at.get_or_insert(self.output.len());
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.output.len()
    }

    pub(crate) fn truncate(&mut self, length: usize) {
        self.output.truncate(length);
        if self.overflow_at.is_some_and(|overflow_at| overflow_at >= length) {
            self.overflow_at = None;
        }
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    pub(crate) fn i8(&mut self, value: i8) {
        self.bytes(&[value as u8]);
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn i16(&mut self, value: i16) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn i32(&mut self, value: i32) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn i64(&mut self, value: i64) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn f32(&mut self, value: f32) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn f64(&mut self, value: f64) {
        self.bytes(&value.to_le_bytes());
    }

    pub(crate) fn uuid(&mut self, value: Uuid) {
        self.bytes(value.as_bytes());
    }

    pub(crate) fn variable1_string(&mut self, value: &str) -> Result<(), LLUdpError> {
        validate_ll_string(value)?;
        let len = value.len().checked_add(1).and_then(|len| u8::try_from(len).ok()).ok_or(LLUdpError::VariableFieldTooLong)?;
        self.u8(len);
        self.bytes(value.as_bytes());
        self.u8(0);
        Ok(())
    }

    pub(crate) fn vector3(&mut self, value: Vec3) {
        self.f32(value.x);
        self.f32(value.y);
        self.f32(value.z);
    }

    pub(crate) fn finite_vector3(&mut self, value: Vec3) -> Result<(), LLUdpError> {
        if !value.is_finite() {
            return Err(LLUdpError::InvalidMessage);
        }
        self.vector3(value);
        Ok(())
    }

    pub(crate) fn finite_vector4(&mut self, value: Vec4) -> Result<(), LLUdpError> {
        if !value.is_finite() {
            return Err(LLUdpError::InvalidMessage);
        }
        self.f32(value.x);
        self.f32(value.y);
        self.f32(value.z);
        self.f32(value.w);
        Ok(())
    }

    pub(crate) fn variable1(&mut self, value: &[u8]) -> Result<(), LLUdpError> {
        self.u8(u8::try_from(value.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        self.bytes(value);
        Ok(())
    }

    pub(crate) fn variable2(&mut self, value: &[u8]) -> Result<(), LLUdpError> {
        self.u16(u16::try_from(value.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        self.bytes(value);
        Ok(())
    }

    pub(crate) fn vector3d(&mut self, value: DVec3) {
        self.f64(value.x);
        self.f64(value.y);
        self.f64(value.z);
    }

    pub(crate) fn quaternion_xyz(&mut self, value: Quat) -> Result<(), LLUdpError> {
        let [x, y, z] = canonical_quaternion_xyz(value)?;
        self.f32(x);
        self.f32(y);
        self.f32(z);
        Ok(())
    }

    pub(crate) fn variable2_string(&mut self, value: &str) -> Result<(), LLUdpError> {
        validate_ll_string(value)?;
        let len = value.len().checked_add(1).and_then(|len| u16::try_from(len).ok()).ok_or(LLUdpError::VariableFieldTooLong)?;
        self.bytes(&len.to_le_bytes());
        self.bytes(value.as_bytes());
        self.bytes(&[0]);
        Ok(())
    }
}

pub(crate) struct Reader<'a> {
    input: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self { input, cursor: 0 }
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], LLUdpError> {
        let end = self.cursor.checked_add(len).ok_or(LLUdpError::TruncatedPacket)?;
        let bytes = self.input.get(self.cursor..end).ok_or(LLUdpError::TruncatedPacket)?;
        self.cursor = end;
        Ok(bytes)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.cursor == self.input.len()
    }

    pub(crate) fn remaining(&self) -> &'a [u8] {
        &self.input[self.cursor..]
    }

    pub(crate) fn position(&self) -> usize {
        self.cursor
    }

    pub(crate) fn skip_remaining(&mut self) {
        self.cursor = self.input.len();
    }

    pub(crate) fn terminal_variable_count(&mut self) -> Result<usize, LLUdpError> {
        if self.is_empty() { Ok(0) } else { Ok(usize::from(self.u8()?)) }
    }

    pub(crate) fn u8(&mut self) -> Result<u8, LLUdpError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn i8(&mut self) -> Result<i8, LLUdpError> {
        Ok(self.u8()? as i8)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, LLUdpError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, LLUdpError> {
        let bytes = self.take(2)?;
        Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, LLUdpError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, LLUdpError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, LLUdpError> {
        let bytes = self.take(8)?;
        Ok(i64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, LLUdpError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, LLUdpError> {
        let bytes = self.take(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn f64(&mut self) -> Result<f64, LLUdpError> {
        let bytes = self.take(8)?;
        Ok(f64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    pub(crate) fn uuid(&mut self) -> Result<Uuid, LLUdpError> {
        Uuid::from_slice(self.take(16)?).map_err(|_| LLUdpError::TruncatedPacket)
    }

    pub(crate) fn vector3(&mut self) -> Result<Vec3, LLUdpError> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    pub(crate) fn vector4(&mut self) -> Result<Vec4, LLUdpError> {
        Ok(Vec4::new(self.f32()?, self.f32()?, self.f32()?, self.f32()?))
    }

    pub(crate) fn variable1(&mut self) -> Result<&'a [u8], LLUdpError> {
        let length = usize::from(self.u8()?);
        self.take(length)
    }

    pub(crate) fn variable2(&mut self) -> Result<&'a [u8], LLUdpError> {
        let length = usize::from(self.u16()?);
        self.take(length)
    }

    pub(crate) fn vector3d(&mut self) -> Result<DVec3, LLUdpError> {
        Ok(DVec3::new(self.f64()?, self.f64()?, self.f64()?))
    }

    pub(crate) fn quaternion(&mut self) -> Result<Quat, LLUdpError> {
        quaternion_from_xyz(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    pub(crate) fn variable1_string(&mut self) -> Result<String, LLUdpError> {
        decode_string(self.variable1()?)
    }

    pub(crate) fn variable2_string(&mut self) -> Result<String, LLUdpError> {
        decode_string(self.variable2()?)
    }
}

fn canonical_quaternion_xyz(value: Quat) -> Result<[f32; 3], LLUdpError> {
    let mut components = [f64::from(value.x), f64::from(value.y), f64::from(value.z), f64::from(value.w)];
    if components.iter().any(|component| !component.is_finite()) {
        return Err(LLUdpError::InvalidMessage);
    }

    let scale = components.iter().map(|component| component.abs()).fold(0.0, f64::max);
    if scale == 0.0 {
        return Err(LLUdpError::InvalidMessage);
    }

    for component in &mut components {
        *component /= scale;
    }
    let length = components.iter().map(|component| component * component).sum::<f64>().sqrt();
    for component in &mut components {
        *component /= length;
    }

    let reverse = if components[3] == 0.0 {
        components[..3].iter().find(|component| **component != 0.0).is_some_and(|component| *component < 0.0)
    } else {
        components[3] < 0.0
    };
    if reverse {
        for component in &mut components {
            *component = -*component;
        }
    }

    let mut xyz = [components[0] as f32, components[1] as f32, components[2] as f32];
    for component in &mut xyz {
        if *component == 0.0 {
            *component = 0.0;
        }
    }
    Ok(xyz)
}

fn validate_ll_string(value: &str) -> Result<(), LLUdpError> {
    if value.contains('\0') {
        return Err(LLUdpError::InvalidMessage);
    }
    Ok(())
}

fn decode_string(bytes: &[u8]) -> Result<String, LLUdpError> {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    str::from_utf8(&bytes[..end]).map(str::to_owned).map_err(|_| LLUdpError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::{Reader, Writer};
    use crate::lludp::LLUdpError;
    use math::Quat;

    fn encode_quaternion(value: Quat) -> Result<Vec<u8>, LLUdpError> {
        let mut output = Vec::new();
        let mut writer = Writer::new(&mut output);
        writer.quaternion_xyz(value)?;
        Ok(output)
    }

    fn decode_quaternion(bytes: &[u8]) -> Quat {
        Reader::new(bytes).quaternion().expect("encoded quaternion is valid")
    }

    fn assert_rotation_equivalent(left: Quat, right: Quat) {
        assert!((left.normalize().dot(right.normalize()).abs() - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn bounded_writer_caps_output() {
        let payload = vec![0x55; 1_000_000];
        let mut output = Vec::new();
        let mut writer = Writer::with_limit(&mut output, 8);
        writer.bytes(&payload);

        assert_eq!(writer.finish(), Err(LLUdpError::DatagramTooLarge));
        assert_eq!(output, [0x55; 8]);
    }

    #[test]
    fn signed_byte_respects_limit() {
        let mut output = Vec::new();
        let mut writer = Writer::with_limit(&mut output, 0);
        writer.i8(-1);

        assert_eq!(writer.finish(), Err(LLUdpError::DatagramTooLarge));
        assert!(output.is_empty());
    }

    #[test]
    fn truncation_discards_overflow() {
        let mut output = Vec::new();
        let mut writer = Writer::with_limit(&mut output, 4);
        writer.bytes(&[1, 2, 3, 4, 5, 6]);
        writer.truncate(2);

        assert_eq!(writer.finish(), Ok(()));
        assert_eq!(output, [1, 2]);
    }

    #[test]
    fn quaternion_xyz_normalizes_and_canonicalizes_negative_w() {
        let input = Quat::from_xyzw(0.2, -0.3, 0.4, -0.84);
        let bytes = encode_quaternion(input).expect("encode finite nonzero rotation");
        let decoded = decode_quaternion(&bytes);

        assert!(decoded.w >= 0.0);
        assert_rotation_equivalent(input, decoded);
    }

    #[test]
    fn quaternion_xyz_gives_equivalent_signs_identical_bytes() {
        for input in [Quat::from_xyzw(0.2, -0.3, 0.4, 0.84), Quat::from_xyzw(-1.0, 2.0, -3.0, 0.0)] {
            let inverse = Quat::from_xyzw(-input.x, -input.y, -input.z, -input.w);
            assert_eq!(encode_quaternion(input), encode_quaternion(inverse));
        }
    }

    #[test]
    fn quaternion_xyz_encodes_identity_and_normalizes_nonunit_input() {
        assert_eq!(encode_quaternion(Quat::IDENTITY).expect("identity is valid"), [0; 12]);
        let nonunit = Quat::from_xyzw(0.0, 3.0, 0.0, 4.0);
        let decoded = decode_quaternion(&encode_quaternion(nonunit).expect("nonunit rotation is valid"));

        assert_rotation_equivalent(nonunit, decoded);
        assert!(decoded.w >= 0.0);
    }

    #[test]
    fn quaternion_xyz_rejects_zero_and_nonfinite_values_without_writing() {
        for invalid in [
            Quat::from_xyzw(0.0, 0.0, 0.0, 0.0),
            Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
            Quat::from_xyzw(0.0, f32::INFINITY, 0.0, 1.0),
            Quat::from_xyzw(0.0, 0.0, 0.0, f32::NEG_INFINITY),
        ] {
            let mut output = vec![0xaa];
            let result = Writer::new(&mut output).quaternion_xyz(invalid);

            assert_eq!(result, Err(LLUdpError::InvalidMessage));
            assert_eq!(output, [0xaa]);
        }
    }
}
