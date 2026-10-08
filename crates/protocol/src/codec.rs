use base::UUID;

use crate::{
    error::UdpError,
    types::{Quaternion, Vector3},
};

fn validate_ll_string(value: &str) -> Result<(), UdpError> {
    if value.contains('\0') {
        return Err(UdpError::InvalidMessage);
    }
    Ok(())
}

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

    pub(crate) fn finish(self) -> Result<(), UdpError> {
        if self.overflow_at.is_some() { Err(UdpError::DatagramTooLarge) } else { Ok(()) }
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

    pub(crate) fn uuid(&mut self, value: UUID) {
        self.bytes(value.as_bytes());
    }

    pub(crate) fn variable1_string(&mut self, value: &str) -> Result<(), UdpError> {
        validate_ll_string(value)?;
        let len = value.len().checked_add(1).and_then(|len| u8::try_from(len).ok()).ok_or(UdpError::VariableFieldTooLong)?;
        self.u8(len);
        self.bytes(value.as_bytes());
        self.u8(0);
        Ok(())
    }

    pub(crate) fn vector3(&mut self, value: Vector3) {
        self.f32(value.x);
        self.f32(value.y);
        self.f32(value.z);
    }

    pub(crate) fn quaternion(&mut self, value: Quaternion) {
        self.f32(value.x);
        self.f32(value.y);
        self.f32(value.z);
    }

    pub(crate) fn variable2_string(&mut self, value: &str) -> Result<(), UdpError> {
        validate_ll_string(value)?;
        let len = value.len().checked_add(1).and_then(|len| u16::try_from(len).ok()).ok_or(UdpError::VariableFieldTooLong)?;
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

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], UdpError> {
        let end = self.cursor.checked_add(len).ok_or(UdpError::TruncatedPacket)?;
        let bytes = self.input.get(self.cursor..end).ok_or(UdpError::TruncatedPacket)?;
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

    pub(crate) fn terminal_variable_count(&mut self) -> Result<usize, UdpError> {
        if self.is_empty() { Ok(0) } else { Ok(usize::from(self.u8()?)) }
    }

    pub(crate) fn u8(&mut self) -> Result<u8, UdpError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn i8(&mut self) -> Result<i8, UdpError> {
        Ok(self.u8()? as i8)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, UdpError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, UdpError> {
        let bytes = self.take(2)?;
        Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, UdpError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, UdpError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, UdpError> {
        let bytes = self.take(8)?;
        Ok(i64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, UdpError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, UdpError> {
        let bytes = self.take(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn f64(&mut self) -> Result<f64, UdpError> {
        let bytes = self.take(8)?;
        Ok(f64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }

    pub(crate) fn uuid(&mut self) -> Result<UUID, UdpError> {
        UUID::from_slice(self.take(16)?).map_err(|_| UdpError::TruncatedPacket)
    }

    pub(crate) fn vector3(&mut self) -> Result<Vector3, UdpError> {
        Ok(Vector3 { x: self.f32()?, y: self.f32()?, z: self.f32()? })
    }

    pub(crate) fn quaternion(&mut self) -> Result<Quaternion, UdpError> {
        Ok(Quaternion { x: self.f32()?, y: self.f32()?, z: self.f32()? })
    }

    pub(crate) fn variable1_string(&mut self) -> Result<String, UdpError> {
        let len = usize::from(self.u8()?);
        let bytes = self.take(len)?;
        decode_string(bytes)
    }

    pub(crate) fn variable2_string(&mut self) -> Result<String, UdpError> {
        let bytes = self.take(2)?;
        let len = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
        let bytes = self.take(len)?;
        decode_string(bytes)
    }
}

fn decode_string(bytes: &[u8]) -> Result<String, UdpError> {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).map(str::to_owned).map_err(|_| UdpError::InvalidUtf8)
}

#[cfg(test)]
mod tests {
    use super::Writer;
    use crate::error::UdpError;

    #[test]
    fn bounded_writer_caps_output() {
        let payload = vec![0x55; 1_000_000];
        let mut output = Vec::new();
        let mut writer = Writer::with_limit(&mut output, 8);
        writer.bytes(&payload);

        assert_eq!(writer.finish(), Err(UdpError::DatagramTooLarge));
        assert_eq!(output, [0x55; 8]);
    }

    #[test]
    fn signed_byte_respects_limit() {
        let mut output = Vec::new();
        let mut writer = Writer::with_limit(&mut output, 0);
        writer.i8(-1);

        assert_eq!(writer.finish(), Err(UdpError::DatagramTooLarge));
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
}
