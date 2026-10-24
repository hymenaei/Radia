use std::{collections::BTreeMap, num::NonZeroU32};

use llsd::{Limits, Map, Value, notation};
use protocol::messages::{GenericStreamingMessage, generic_streaming_message::MAX_DATA_BYTES};
use world::{GLTF_TEXTURE_INFO_COUNT, GltfAlphaMode, GltfMaterialOverride, GltfMaterialOverrides, GltfTextureTransform, MAX_SURFACE_FACES};

const METHOD: u16 = 0x4175;
const LLSD_LIMITS: Limits = Limits {
    max_bytes: MAX_DATA_BYTES,
    max_depth: 8,
    max_values: 2048,
    max_container_items: 256,
    max_string_bytes: 512,
    max_map_key_bytes: 16,
    max_binary_bytes: 0,
};

pub(super) struct Decoded {
    pub(super) local_id: NonZeroU32,
    pub(super) overrides: GltfMaterialOverrides,
}

pub(super) fn decode_message(message: &GenericStreamingMessage) -> Option<Result<Decoded, ()>> {
    if message.method != METHOD {
        return None;
    }

    Some(decode_payload(&message.data))
}

fn decode_payload(bytes: &[u8]) -> Result<Decoded, ()> {
    let payload: Value = notation::from_slice_with_limits(bytes, LLSD_LIMITS).map_err(|_| ())?;
    let Value::Map(payload) = payload else { return Err(()) };
    let local_id = match payload.get("id") {
        Some(Value::Integer(value)) => NonZeroU32::new(*value as u32).ok_or(())?,
        _ => return Err(()),
    };
    let faces = match payload.get("te") {
        Some(Value::Array(faces)) => faces,
        _ => return Err(()),
    };
    let materials = match payload.get("od") {
        Some(Value::Array(materials)) if materials.len() == faces.len() => materials,
        _ => return Err(()),
    };

    let mut overrides = BTreeMap::new();
    for (face, material) in faces.iter().zip(materials).take(MAX_SURFACE_FACES) {
        let face = face_index(face)?;
        let material = match material {
            Value::Map(material) => decode_material(material)?,
            Value::Undefined => GltfMaterialOverride::default(),
            _ => return Err(()),
        };
        overrides.insert(face, material);
    }
    let overrides = GltfMaterialOverrides::new(overrides.into_iter().collect()).map_err(|_| ())?;
    Ok(Decoded { local_id, overrides })
}

fn face_index(value: &Value) -> Result<u8, ()> {
    match value {
        Value::Integer(index) if (0..MAX_SURFACE_FACES as i32).contains(index) => Ok(*index as u8),
        _ => Err(()),
    }
}

fn decode_material(fields: &Map) -> Result<GltfMaterialOverride, ()> {
    let mut material = GltfMaterialOverride::default();
    if let Some(value) = fields.get("tex") {
        let Value::Array(textures) = value else { return Err(()) };
        if textures.len() > GLTF_TEXTURE_INFO_COUNT {
            return Err(());
        }
        for (index, texture) in textures.iter().enumerate() {
            let Value::Uuid(texture) = texture else { return Err(()) };
            material.textures[index] = Some(*texture);
        }
    }
    if let Some(value) = fields.get("bc") {
        material.base_color = Some(vector(value)?);
    }
    if let Some(value) = fields.get("ec") {
        material.emissive = Some(vector(value)?);
    }
    if let Some(value) = fields.get("mf") {
        material.metallic = Some(number(value)?);
    }
    if let Some(value) = fields.get("rf") {
        material.roughness = Some(number(value)?);
    }
    if let Some(value) = fields.get("am") {
        material.alpha_mode = Some(match value {
            Value::Integer(0) => GltfAlphaMode::Opaque,
            Value::Integer(1) => GltfAlphaMode::Blend,
            Value::Integer(2) => GltfAlphaMode::Mask,
            _ => return Err(()),
        });
    }
    if let Some(value) = fields.get("ac") {
        material.alpha_cutoff = Some(number(value)?);
    }
    if let Some(value) = fields.get("ds") {
        material.double_sided = Some(match value {
            Value::Boolean(value) => *value,
            _ => return Err(()),
        });
    }
    if let Some(value) = fields.get("ti") {
        let Value::Array(transforms) = value else { return Err(()) };
        if transforms.len() > GLTF_TEXTURE_INFO_COUNT {
            return Err(());
        }
        for (index, transform) in transforms.iter().enumerate() {
            let Value::Map(transform) = transform else { return Err(()) };
            let mut decoded = GltfTextureTransform::default();
            if let Some(value) = transform.get("o") {
                decoded.offset = Some(vector(value)?);
            }
            if let Some(value) = transform.get("s") {
                decoded.scale = Some(vector(value)?);
            }
            if let Some(value) = transform.get("r") {
                decoded.rotation = Some(number(value)?);
            }
            material.texture_transforms[index] = decoded;
        }
    }
    Ok(material)
}

fn vector<const N: usize>(value: &Value) -> Result<[f32; N], ()> {
    let Value::Array(values) = value else { return Err(()) };
    let values: Vec<_> = values.iter().map(number).collect::<Result<_, _>>()?;
    values.try_into().map_err(|_| ())
}

fn number(value: &Value) -> Result<f32, ()> {
    let value = match value {
        Value::Integer(value) => *value as f64,
        Value::Real(value) => *value,
        _ => return Err(()),
    };
    let value = value as f32;
    value.is_finite().then_some(value).ok_or(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use llsd::{Value, notation};
    use uuid::Uuid;
    use world::{GltfAlphaMode, GltfMaterialOverride};

    use protocol::messages::generic_streaming_message::MAX_DATA_BYTES;

    use super::decode_payload;

    fn payload(id: i32, faces: Vec<Value>, materials: Vec<Value>) -> Vec<u8> {
        let mut map = BTreeMap::new();
        map.insert("id".to_owned(), Value::Integer(id));
        map.insert("te".to_owned(), Value::Array(faces));
        map.insert("od".to_owned(), Value::Array(materials));
        notation::to_vec(&Value::Map(map)).expect("serialize LLSD payload")
    }

    fn material(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
        Value::Map(fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
    }

    #[test]
    fn decodes_material_fields() {
        let data = material([
            ("bc", Value::Array(vec![Value::Real(0.1), Value::Real(0.2), Value::Real(0.3), Value::Real(1.0)])),
            ("am", Value::Integer(2)),
        ]);
        let decoded = decode_payload(&payload(7, vec![Value::Integer(4)], vec![data])).expect("valid override");

        assert_eq!(decoded.local_id.get(), 7);
        assert_eq!(decoded.overrides.face(4).unwrap().alpha_mode, Some(GltfAlphaMode::Mask));
        assert_eq!(decoded.overrides.face(4).unwrap().base_color, Some([0.1, 0.2, 0.3, 1.0]));
    }

    #[test]
    fn empty_faces_clear_snapshot() {
        let decoded = decode_payload(&payload(7, Vec::new(), Vec::new())).expect("empty snapshot");

        assert!(decoded.overrides.is_empty());
    }

    #[test]
    fn duplicate_face_uses_last_value() {
        let decoded = decode_payload(&payload(
            7,
            vec![Value::Integer(4), Value::Integer(4)],
            vec![material([("mf", Value::Real(0.1))]), material([("mf", Value::Real(0.8))])],
        ))
        .expect("last face value wins");

        assert_eq!(decoded.overrides.faces().len(), 1);
        assert_eq!(decoded.overrides.face(4).unwrap().metallic, Some(0.8));
    }

    #[test]
    fn rejects_bad_faces_and_alignment() {
        assert!(decode_payload(&payload(7, vec![Value::Integer(45)], vec![material([])])).is_err());
        assert!(decode_payload(&payload(7, vec![Value::Integer(4)], Vec::new())).is_err());
    }

    #[test]
    fn rejects_bad_vectors_and_texture_counts() {
        let bad_vector = material([("bc", Value::Array(vec![Value::Real(1.0)]))]);
        assert!(decode_payload(&payload(7, vec![Value::Integer(4)], vec![bad_vector])).is_err());

        let too_many = material([("tex", Value::Array(vec![Value::Uuid(Uuid::nil()); 5]))]);
        assert!(decode_payload(&payload(7, vec![Value::Integer(4)], vec![too_many])).is_err());
    }

    #[test]
    fn rejects_non_finite_values_and_oversized_payload() {
        let non_finite = material([("rf", Value::Real(f64::MAX))]);
        assert!(decode_payload(&payload(7, vec![Value::Integer(4)], vec![non_finite])).is_err());
        assert!(decode_payload(&vec![b' '; MAX_DATA_BYTES + 1]).is_err());
    }

    #[test]
    fn preserves_signed_local_id_bits() {
        for (wire_id, expected) in [(i32::MAX, i32::MAX as u32), (i32::MIN, 0x8000_0000), (-1, u32::MAX)] {
            let decoded = decode_payload(&payload(wire_id, Vec::new(), Vec::new())).expect("valid local id");

            assert_eq!(decoded.local_id.get(), expected);
        }
    }

    #[test]
    fn rejects_zero_local_id() {
        assert!(decode_payload(&payload(0, Vec::new(), Vec::new())).is_err());
    }

    #[test]
    fn ignores_unsupported_material_keys() {
        let decoded = decode_payload(&payload(7, vec![Value::Integer(4)], vec![material([("future", Value::Integer(3))])]))
            .expect("unknown keys are forward-compatible");

        assert_eq!(decoded.overrides.face(4), Some(&GltfMaterialOverride::default()));
    }

    #[test]
    fn undefined_material_is_default() {
        let decoded = decode_payload(&payload(7, vec![Value::Integer(4)], vec![Value::Undefined])).expect("undefined material is valid");

        assert_eq!(decoded.overrides.face(4), Some(&GltfMaterialOverride::default()));
    }

    #[test]
    fn caps_faces_at_texture_entry_limit() {
        let faces = (0..46).map(Value::Integer).collect();
        let materials = (0..46).map(|_| material([])).collect();
        let decoded = decode_payload(&payload(7, faces, materials)).expect("excess faces are ignored");

        assert_eq!(decoded.overrides.faces().len(), 45);
    }
}
