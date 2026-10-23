use uuid::Uuid;

use crate::{lludp::codec::Reader, types::texture_entry::MAX_TEXTURE_FACES};

pub const MAX_RENDER_MATERIAL_BINDINGS: usize = MAX_TEXTURE_FACES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderMaterialBinding {
    pub face_index: u8,
    pub material_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderMaterialError {
    Malformed,
}

pub fn decode_render_material_bindings(bytes: &[u8]) -> Result<Vec<RenderMaterialBinding>, RenderMaterialError> {
    let mut reader = Reader::new(bytes);
    let count = usize::from(reader.u8().map_err(|_| RenderMaterialError::Malformed)?);
    let expected_length = 1 + count * 17;
    if bytes.len() != expected_length {
        return Err(RenderMaterialError::Malformed);
    }

    let mut seen = [false; MAX_TEXTURE_FACES];
    let mut bindings = Vec::with_capacity(count.min(MAX_RENDER_MATERIAL_BINDINGS));
    for _ in 0..count {
        let face_index = reader.u8().map_err(|_| RenderMaterialError::Malformed)?;
        let index = usize::from(face_index);
        let material_id = reader.uuid().map_err(|_| RenderMaterialError::Malformed)?;
        if index < MAX_TEXTURE_FACES && !std::mem::replace(&mut seen[index], true) {
            bindings.push(RenderMaterialBinding { face_index, material_id });
        }
    }

    Ok(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn binding(bytes: &mut Vec<u8>, face_index: u8, material_id: Uuid) {
        bytes.push(face_index);
        bytes.extend_from_slice(material_id.as_bytes());
    }

    #[test]
    fn decodes_sparse_render_material_bindings() {
        let first = uuid(0x102);
        let last = uuid(0x103);
        let mut bytes = vec![2];
        binding(&mut bytes, 0, first);
        binding(&mut bytes, 44, last);

        assert_eq!(
            decode_render_material_bindings(&bytes).unwrap(),
            [RenderMaterialBinding { face_index: 0, material_id: first }, RenderMaterialBinding { face_index: 44, material_id: last }]
        );
    }

    #[test]
    fn empty_bindings_accept_nil_material_ids() {
        assert_eq!(decode_render_material_bindings(&[0]).unwrap(), []);

        let mut bytes = vec![1];
        binding(&mut bytes, 2, Uuid::nil());
        assert_eq!(decode_render_material_bindings(&bytes).unwrap()[0].material_id, Uuid::nil());
    }

    #[test]
    fn bindings_validate_and_keep_first() {
        assert_eq!(decode_render_material_bindings(&[]), Err(RenderMaterialError::Malformed));
        assert_eq!(decode_render_material_bindings(&[1]), Err(RenderMaterialError::Malformed));

        let mut bytes = vec![3];
        binding(&mut bytes, 45, uuid(0x103));
        binding(&mut bytes, 4, uuid(0x104));
        binding(&mut bytes, 4, uuid(0x105));
        let decoded = decode_render_material_bindings(&bytes).unwrap();
        assert_eq!(decoded, [RenderMaterialBinding { face_index: 4, material_id: uuid(0x104) }]);

        let bytes = vec![0, 0];
        assert_eq!(decode_render_material_bindings(&bytes), Err(RenderMaterialError::Malformed));
    }

    #[test]
    fn accepts_u8_count_and_caps_faces() {
        let mut bytes = vec![u8::MAX];
        for index in 0..usize::from(u8::MAX) {
            let face_index = (index % MAX_TEXTURE_FACES) as u8;
            binding(&mut bytes, face_index, uuid(0x1000 + index as u128));
        }

        let decoded = decode_render_material_bindings(&bytes).unwrap();
        assert_eq!(decoded.len(), MAX_RENDER_MATERIAL_BINDINGS);
        assert_eq!(decoded[0], RenderMaterialBinding { face_index: 0, material_id: uuid(0x1000) });
        assert_eq!(decoded[44], RenderMaterialBinding { face_index: 44, material_id: uuid(0x102c) });
    }
}
