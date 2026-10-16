//! Path and profile values embedded in object-update payloads.

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
};

/// Path and profile parameters carried by full and compressed object updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectShape {
    pub path_curve: u8,
    pub profile_curve: u8,
    pub path_begin: u16,
    pub path_end: u16,
    pub path_scale_x: u8,
    pub path_scale_y: u8,
    pub path_shear_x: u8,
    pub path_shear_y: u8,
    pub path_twist: i8,
    pub path_twist_begin: i8,
    pub path_radius_offset: i8,
    pub path_taper_x: i8,
    pub path_taper_y: i8,
    pub path_revolutions: u8,
    pub path_skew: i8,
    pub profile_begin: u16,
    pub profile_end: u16,
    pub profile_hollow: u16,
}

impl ObjectShape {
    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            path_curve: reader.u8()?,
            profile_curve: reader.u8()?,
            path_begin: reader.u16()?,
            path_end: reader.u16()?,
            path_scale_x: reader.u8()?,
            path_scale_y: reader.u8()?,
            path_shear_x: reader.u8()?,
            path_shear_y: reader.u8()?,
            path_twist: reader.i8()?,
            path_twist_begin: reader.i8()?,
            path_radius_offset: reader.i8()?,
            path_taper_x: reader.i8()?,
            path_taper_y: reader.i8()?,
            path_revolutions: reader.u8()?,
            path_skew: reader.i8()?,
            profile_begin: reader.u16()?,
            profile_end: reader.u16()?,
            profile_hollow: reader.u16()?,
        })
    }

    pub(crate) fn encode(self, writer: &mut Writer<'_>) {
        writer.u8(self.path_curve);
        writer.u8(self.profile_curve);
        writer.u16(self.path_begin);
        writer.u16(self.path_end);
        writer.u8(self.path_scale_x);
        writer.u8(self.path_scale_y);
        writer.u8(self.path_shear_x);
        writer.u8(self.path_shear_y);
        writer.i8(self.path_twist);
        writer.i8(self.path_twist_begin);
        writer.i8(self.path_radius_offset);
        writer.i8(self.path_taper_x);
        writer.i8(self.path_taper_y);
        writer.u8(self.path_revolutions);
        writer.i8(self.path_skew);
        writer.u16(self.profile_begin);
        writer.u16(self.profile_end);
        writer.u16(self.profile_hollow);
    }

    pub(crate) fn decode_compressed(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let path_curve = reader.u8()?;
        let path_begin = reader.u16()?;
        let path_end = reader.u16()?;
        let path_scale_x = reader.u8()?;
        let path_scale_y = reader.u8()?;
        let path_shear_x = reader.u8()?;
        let path_shear_y = reader.u8()?;
        let path_twist = reader.i8()?;
        let path_twist_begin = reader.i8()?;
        let path_radius_offset = reader.i8()?;
        let path_taper_x = reader.i8()?;
        let path_taper_y = reader.i8()?;
        let path_revolutions = reader.u8()?;
        let path_skew = reader.i8()?;
        let profile_curve = reader.u8()?;
        let profile_begin = reader.u16()?;
        let profile_end = reader.u16()?;
        let profile_hollow = reader.u16()?;
        Ok(Self {
            path_curve,
            profile_curve,
            path_begin,
            path_end,
            path_scale_x,
            path_scale_y,
            path_shear_x,
            path_shear_y,
            path_twist,
            path_twist_begin,
            path_radius_offset,
            path_taper_x,
            path_taper_y,
            path_revolutions,
            path_skew,
            profile_begin,
            profile_end,
            profile_hollow,
        })
    }

    pub(crate) fn encode_compressed(self, writer: &mut Writer<'_>) {
        writer.u8(self.path_curve);
        writer.u16(self.path_begin);
        writer.u16(self.path_end);
        writer.u8(self.path_scale_x);
        writer.u8(self.path_scale_y);
        writer.u8(self.path_shear_x);
        writer.u8(self.path_shear_y);
        writer.i8(self.path_twist);
        writer.i8(self.path_twist_begin);
        writer.i8(self.path_radius_offset);
        writer.i8(self.path_taper_x);
        writer.i8(self.path_taper_y);
        writer.u8(self.path_revolutions);
        writer.i8(self.path_skew);
        writer.u8(self.profile_curve);
        writer.u16(self.profile_begin);
        writer.u16(self.profile_end);
        writer.u16(self.profile_hollow);
    }
}

#[cfg(test)]
mod tests {
    use super::ObjectShape;
    use crate::{
        LLUdpError,
        lludp::codec::{Reader, Writer},
    };

    fn shape() -> ObjectShape {
        ObjectShape {
            path_curve: 1,
            profile_curve: 2,
            path_begin: 3,
            path_end: 4,
            path_scale_x: 5,
            path_scale_y: 6,
            path_shear_x: 7,
            path_shear_y: 8,
            path_twist: -9,
            path_twist_begin: 10,
            path_radius_offset: -11,
            path_taper_x: 12,
            path_taper_y: -13,
            path_revolutions: 14,
            path_skew: -15,
            profile_begin: 16,
            profile_end: 17,
            profile_hollow: 18,
        }
    }

    #[test]
    fn regular_shape_round_trips() {
        let value = shape();
        let mut bytes = Vec::new();
        let mut writer = Writer::with_limit(&mut bytes, usize::MAX);
        value.encode(&mut writer);
        writer.finish().unwrap();

        assert_eq!(bytes.len(), 23);
        assert_eq!(ObjectShape::decode(&mut Reader::new(&bytes)).unwrap(), value);
    }

    #[test]
    fn compressed_shape_round_trips() {
        let value = shape();
        let mut bytes = Vec::new();
        let mut writer = Writer::with_limit(&mut bytes, usize::MAX);
        value.encode_compressed(&mut writer);
        writer.finish().unwrap();

        assert_eq!(bytes.len(), 23);
        assert_eq!(ObjectShape::decode_compressed(&mut Reader::new(&bytes)).unwrap(), value);
    }

    #[test]
    fn truncated_shape_is_rejected() {
        let mut bytes = Vec::new();
        let mut writer = Writer::with_limit(&mut bytes, usize::MAX);
        shape().encode(&mut writer);
        writer.finish().unwrap();

        assert_eq!(ObjectShape::decode(&mut Reader::new(&bytes[..22])), Err(LLUdpError::TruncatedPacket));
    }
}
