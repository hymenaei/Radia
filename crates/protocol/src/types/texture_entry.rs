use uuid::Uuid;

use crate::lludp::codec::Reader;

pub const MAX_TEXTURE_FACES: usize = 45;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureFace {
    pub texture_id: Uuid,
    pub color: [u8; 4],
    pub scale_s: f32,
    pub scale_t: f32,
    pub offset_s: f32,
    pub offset_t: f32,
    pub rotation: f32,
    pub bump_shiny_fullbright: u8,
    pub media_texgen: u8,
    pub glow: f32,
    pub material_id: Uuid,
}

impl Default for TextureFace {
    fn default() -> Self {
        Self {
            texture_id: Uuid::nil(),
            color: [u8::MAX; 4],
            scale_s: 1.0,
            scale_t: 1.0,
            offset_s: 0.0,
            offset_t: 0.0,
            rotation: 0.0,
            bump_shiny_fullbright: 0,
            media_texgen: 0,
            glow: 0.0,
            material_id: Uuid::nil(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextureEntry {
    default_face: TextureFace,
    overrides: Vec<(u8, TextureFace)>,
}

impl TextureEntry {
    pub const fn default_face(&self) -> &TextureFace {
        &self.default_face
    }

    pub fn face(&self, index: u8) -> Option<&TextureFace> {
        if usize::from(index) >= MAX_TEXTURE_FACES {
            return None;
        }
        Some(
            self.overrides
                .binary_search_by_key(&index, |(face_index, _)| *face_index)
                .map_or(&self.default_face, |slot| &self.overrides[slot].1),
        )
    }

    pub fn overrides(&self) -> impl Iterator<Item = (u8, &TextureFace)> {
        self.overrides.iter().map(|(index, face)| (*index, face))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureEntryError {
    Malformed,
}

pub fn decode_texture_entry(bytes: &[u8]) -> Result<Option<TextureEntry>, TextureEntryError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let mut reader = Reader::new(bytes);
    let mut faces = [TextureFace::default(); MAX_TEXTURE_FACES];
    let texture_id = required_field(&mut reader, &mut faces, read_uuid, |face, value| face.texture_id = value)?;
    let color = required_field(&mut reader, &mut faces, read_color, |face, value| face.color = value)?;
    let scale_s = required_field(&mut reader, &mut faces, read_finite_f32, |face, value| face.scale_s = value)?;
    let scale_t = required_field(&mut reader, &mut faces, read_finite_f32, |face, value| face.scale_t = value)?;
    let offset_s = required_field(&mut reader, &mut faces, read_i16, |face, value| face.offset_s = f32::from(value))?;
    let offset_t = required_field(&mut reader, &mut faces, read_i16, |face, value| face.offset_t = f32::from(value))?;
    let rotation = required_field(&mut reader, &mut faces, read_i16, |face, value| face.rotation = f32::from(value))?;
    let bump_shiny_fullbright = required_field(&mut reader, &mut faces, read_u8, |face, value| {
        face.bump_shiny_fullbright = value;
    })?;
    let media_texgen = required_field(&mut reader, &mut faces, read_u8, |face, value| face.media_texgen = value)?;
    let glow = required_field(&mut reader, &mut faces, read_u8, |face, value| face.glow = f32::from(value))?;
    let mut default_face = TextureFace {
        texture_id,
        color,
        scale_s,
        scale_t,
        offset_s: f32::from(offset_s),
        offset_t: f32::from(offset_t),
        rotation: f32::from(rotation),
        bump_shiny_fullbright,
        media_texgen,
        glow: f32::from(glow),
        material_id: Uuid::nil(),
    };

    decode_material_ids(&mut reader, &mut faces, &mut default_face);
    convert_wire_values(&mut default_face);
    for face in &mut faces {
        convert_wire_values(face);
    }
    let overrides =
        faces.into_iter().enumerate().filter_map(|(index, face)| (face != default_face).then_some((index as u8, face))).collect();

    Ok(Some(TextureEntry { default_face, overrides }))
}

fn convert_wire_values(face: &mut TextureFace) {
    face.color = face.color.map(|byte| u8::MAX - byte);
    face.offset_s /= 32767.0;
    face.offset_t /= 32767.0;
    face.rotation = (face.rotation / 32768.0) * std::f32::consts::TAU;
    face.glow /= 255.0;
}

fn required_field<'a, T: Copy>(
    reader: &mut Reader<'a>,
    faces: &mut [TextureFace; MAX_TEXTURE_FACES],
    read_value: impl Fn(&mut Reader<'a>) -> Option<T>,
    apply_value: impl Fn(&mut TextureFace, T),
) -> Result<T, TextureEntryError> {
    let default = read_value(reader).ok_or(TextureEntryError::Malformed)?;
    for face in faces.iter_mut() {
        apply_value(face, default);
    }
    loop {
        let mask = read_mask(reader)?;
        if mask == 0 {
            return Ok(default);
        }
        let value = read_value(reader).ok_or(TextureEntryError::Malformed)?;
        for (index, face) in faces.iter_mut().enumerate() {
            if mask & (1_u64 << index) != 0 {
                apply_value(face, value);
            }
        }
    }
}

fn read_mask(reader: &mut Reader<'_>) -> Result<u64, TextureEntryError> {
    let mut mask = 0_u64;
    for group in 0..7 {
        let byte = reader.u8().map_err(|_| TextureEntryError::Malformed)?;
        mask = (mask << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok(mask);
        }
        if group == 6 {
            return Err(TextureEntryError::Malformed);
        }
    }
    Err(TextureEntryError::Malformed)
}

fn decode_material_ids(reader: &mut Reader<'_>, faces: &mut [TextureFace; MAX_TEXTURE_FACES], default_face: &mut TextureFace) {
    let default_id = reader.uuid().ok();
    let Some(default_id) = default_id else {
        clear_material_ids(faces, default_face);
        return;
    };

    default_face.material_id = default_id;
    for face in faces.iter_mut() {
        face.material_id = default_id;
    }

    loop {
        if reader.is_empty() {
            return;
        }
        let mask = match read_mask(reader) {
            Ok(mask) => mask,
            Err(_) => {
                clear_material_ids(faces, default_face);
                return;
            }
        };
        if mask == 0 {
            return;
        }
        let Some(value) = reader.uuid().ok() else {
            clear_material_ids(faces, default_face);
            return;
        };
        for (index, face) in faces.iter_mut().enumerate() {
            if mask & (1_u64 << index) != 0 {
                face.material_id = value;
            }
        }
    }
}

fn clear_material_ids(faces: &mut [TextureFace; MAX_TEXTURE_FACES], default_face: &mut TextureFace) {
    default_face.material_id = Uuid::nil();
    for face in faces.iter_mut() {
        face.material_id = Uuid::nil();
    }
}

fn read_uuid(reader: &mut Reader<'_>) -> Option<Uuid> {
    reader.uuid().ok()
}

fn read_color(reader: &mut Reader<'_>) -> Option<[u8; 4]> {
    reader.take(4).ok()?.try_into().ok()
}

fn read_finite_f32(reader: &mut Reader<'_>) -> Option<f32> {
    let value = reader.f32().ok()?;
    value.is_finite().then_some(value)
}

fn read_i16(reader: &mut Reader<'_>) -> Option<i16> {
    reader.i16().ok()
}

fn read_u8(reader: &mut Reader<'_>) -> Option<u8> {
    reader.u8().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_wire_entry() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10).as_bytes()[..]);
        bytes.push(0);
        bytes.extend_from_slice(&[0, 0x10, 0x80, 0xff]);
        bytes.push(0);
        bytes.extend_from_slice(&2.0_f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0.5_f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&32767_i16.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&(-32768_i16).to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&8192_i16.to_le_bytes());
        bytes.push(0);
        bytes.push(0b1010_0101);
        bytes.push(0);
        bytes.push(0b0000_0101);
        bytes.push(0);
        bytes.push(128);
        bytes.push(0);
        bytes
    }

    fn uuid_bytes(value: u128) -> [u8; 16] {
        *Uuid::from_u128(value).as_bytes()
    }

    #[test]
    fn texture_entry_defaults_and_conversions() {
        let bytes = default_wire_entry();
        let decoded = decode_texture_entry(&bytes).unwrap().unwrap();
        let face = decoded.face(44).unwrap();

        assert_eq!(face.texture_id, Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10));
        assert_eq!(face.color, [255, 239, 127, 0]);
        assert_eq!((face.scale_s, face.scale_t), (2.0, 0.5));
        assert_eq!(face.offset_s, 1.0);
        assert!(face.offset_t < -1.0);
        assert!((face.rotation - std::f32::consts::FRAC_PI_2).abs() < 0.0001);
        assert_eq!(face.bump_shiny_fullbright, 0b1010_0101);
        assert_eq!(face.media_texgen, 0b0000_0101);
        assert_eq!(face.glow, 128.0 / 255.0);
        assert_eq!(face.material_id, Uuid::nil());
        assert_eq!(decoded.overrides().count(), 0);
    }

    #[test]
    fn face_masks_apply_shared_values() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&uuid_bytes(1));
        bytes.push(0xc0);
        bytes.push(0x80);
        bytes.push(0x80);
        bytes.push(0x80);
        bytes.push(0x80);
        bytes.push(0x80);
        bytes.push(0x06);
        bytes.extend_from_slice(&uuid_bytes(2));
        bytes.push(0);
        bytes.extend_from_slice(&[255, 255, 255, 255]);
        bytes.push(0);
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.push(0);
        for _ in 0..3 {
            bytes.extend_from_slice(&0_i16.to_le_bytes());
            bytes.push(0);
        }
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

        let decoded = decode_texture_entry(&bytes).unwrap().unwrap();

        assert_eq!(decoded.face(0).unwrap().texture_id, Uuid::from_u128(1));
        assert_eq!(decoded.face(1).unwrap().texture_id, Uuid::from_u128(2));
        assert_eq!(decoded.face(2).unwrap().texture_id, Uuid::from_u128(2));
        assert_eq!(decoded.face(44).unwrap().texture_id, Uuid::from_u128(1));
    }

    #[test]
    fn decodes_face_44_override() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&uuid_bytes(1));
        bytes.extend_from_slice(&[0x84, 0x80, 0x80, 0x80, 0x80, 0x80, 0]);
        bytes.extend_from_slice(&uuid_bytes(2));
        bytes.push(0);
        bytes.extend_from_slice(&[255, 255, 255, 255]);
        bytes.push(0);
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.push(0);
        for _ in 0..3 {
            bytes.extend_from_slice(&0_i16.to_le_bytes());
            bytes.push(0);
        }
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

        let decoded = decode_texture_entry(&bytes).unwrap().unwrap();

        assert_eq!(decoded.face(44).unwrap().texture_id, Uuid::from_u128(2));
        assert_eq!(decoded.face(45), None);
        assert_eq!(decoded.overrides().map(|(index, _)| index).collect::<Vec<_>>(), [44]);
    }

    #[test]
    fn rejects_bad_fields_and_non_finite_scales() {
        let bytes = default_wire_entry();
        assert_eq!(decode_texture_entry(&[]), Ok(None));
        for end in [15, 16, 17, 22, 23, 27, 28, 32, 33, 37, 38, 40, 41, 43, 46] {
            assert!(decode_texture_entry(&bytes[..end]).is_err(), "prefix {end}");
        }

        let mut non_finite = default_wire_entry();
        non_finite[18..22].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(decode_texture_entry(&non_finite), Err(TextureEntryError::Malformed));
    }

    #[test]
    fn bad_material_ids_clear_only_materials() {
        let bytes = default_wire_entry();
        let decoded = decode_texture_entry(&bytes).unwrap().unwrap();
        assert_eq!(decoded.face(0).unwrap().material_id, Uuid::nil());

        let mut malformed_tail = bytes;
        malformed_tail.extend_from_slice(&uuid_bytes(5));
        malformed_tail.extend_from_slice(&[0x80]);
        let decoded = decode_texture_entry(&malformed_tail).unwrap().unwrap();
        assert_eq!(decoded.face(0).unwrap().texture_id, Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10));
        assert_eq!(decoded.face(0).unwrap().material_id, Uuid::nil());
    }

    #[test]
    fn material_exceptions_and_bad_masks() {
        let mut bytes = default_wire_entry();
        bytes.extend_from_slice(&uuid_bytes(6));
        bytes.push(1);
        bytes.extend_from_slice(&uuid_bytes(7));
        bytes.push(0);
        let decoded = decode_texture_entry(&bytes).unwrap().unwrap();
        assert_eq!(decoded.face(0).unwrap().material_id, Uuid::from_u128(7));
        assert_eq!(decoded.face(1).unwrap().material_id, Uuid::from_u128(6));

        let mut unterminated = vec![0; 16];
        unterminated.extend_from_slice(&[0x80; 7]);
        assert_eq!(decode_texture_entry(&unterminated), Err(TextureEntryError::Malformed));

        let mut missing_exception_value = vec![0; 16];
        missing_exception_value.extend_from_slice(&[1]);
        assert_eq!(decode_texture_entry(&missing_exception_value), Err(TextureEntryError::Malformed));
    }
}
