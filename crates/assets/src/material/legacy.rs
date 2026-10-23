use llsd::{Limits, Value, binary};

use super::{MaterialDescription, MaterialTexture, TextureTransform, texture_ref};
use crate::Failure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAlphaMode {
    Opaque,
    Blend,
    Mask,
    Emissive,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyMaterial {
    pub diffuse_color: [f32; 4],
    pub diffuse: MaterialTexture,
    pub texgen: TexGen,
    pub fullbright: bool,
    pub bump: u8,
    pub shiny: u8,
    pub glow: f32,
    pub normal: MaterialTexture,
    pub specular: MaterialTexture,
    pub specular_color: [u8; 4],
    pub specular_exponent: u8,
    pub environment_intensity: u8,
    pub alpha_mode: LegacyAlphaMode,
    pub alpha_cutoff: f32,
}

impl Default for LegacyMaterial {
    fn default() -> Self {
        Self {
            diffuse_color: [1.0; 4],
            diffuse: MaterialTexture::default(),
            texgen: TexGen::Default,
            fullbright: false,
            bump: 0,
            shiny: 0,
            glow: 0.0,
            normal: MaterialTexture::default(),
            specular: MaterialTexture::default(),
            specular_color: [255; 4],
            specular_exponent: 51,
            environment_intensity: 0,
            alpha_mode: LegacyAlphaMode::Blend,
            alpha_cutoff: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TexGen {
    #[default]
    Default,
    Planar,
    Unsupported(u8),
}

pub(crate) fn decode(bytes: &[u8]) -> Result<MaterialDescription, Failure> {
    let limits = Limits {
        max_bytes: 4 * 1024 * 1024,
        max_depth: 16,
        max_values: 4096,
        max_container_items: 512,
        max_string_bytes: 2 * 1024 * 1024,
        max_map_key_bytes: 128,
        max_binary_bytes: 2 * 1024 * 1024,
    };
    let Value::Map(fields) = binary::from_slice_with_limits(bytes, limits).map_err(|_| Failure::Malformed)? else {
        return Err(Failure::Malformed);
    };
    let byte = |key| match fields.get(key) {
        Some(Value::Integer(value)) => u8::try_from(*value).map_err(|_| Failure::Malformed),
        _ => Err(Failure::Malformed),
    };
    let alpha_mode = match byte("DiffuseAlphaMode")? {
        0 => LegacyAlphaMode::Opaque,
        1 => LegacyAlphaMode::Blend,
        2 => LegacyAlphaMode::Mask,
        3 => LegacyAlphaMode::Emissive,
        _ => return Err(Failure::Unsupported),
    };
    let fixed = |key| match fields.get(key) {
        Some(Value::Integer(value)) => Ok(*value as f32 / 10_000.0),
        _ => Err(Failure::Malformed),
    };
    let texture = |key| match fields.get(key) {
        Some(Value::Uuid(id)) => Ok(texture_ref(*id)),
        _ => Err(Failure::Malformed),
    };
    let Some(Value::Array(color)) = fields.get("SpecColor") else { return Err(Failure::Malformed) };
    let mut specular_color = [0; 4];
    if color.len() != specular_color.len() {
        return Err(Failure::Malformed);
    }
    for (dest, value) in specular_color.iter_mut().zip(color) {
        let Value::Integer(value) = value else { return Err(Failure::Malformed) };
        *dest = u8::try_from(*value).map_err(|_| Failure::Malformed)?;
    }
    Ok(MaterialDescription::Legacy(LegacyMaterial {
        normal: MaterialTexture {
            texture: texture("NormMap")?,
            transform: TextureTransform {
                offset: [fixed("NormOffsetX")?, fixed("NormOffsetY")?],
                scale: [fixed("NormRepeatX")?, fixed("NormRepeatY")?],
                rotation: fixed("NormRotation")?,
            },
            ..MaterialTexture::default()
        },
        specular: MaterialTexture {
            texture: texture("SpecMap")?,
            transform: TextureTransform {
                offset: [fixed("SpecOffsetX")?, fixed("SpecOffsetY")?],
                scale: [fixed("SpecRepeatX")?, fixed("SpecRepeatY")?],
                rotation: fixed("SpecRotation")?,
            },
            ..MaterialTexture::default()
        },
        specular_color,
        specular_exponent: byte("SpecExp")?,
        environment_intensity: byte("EnvIntensity")?,
        alpha_mode,
        alpha_cutoff: f32::from(byte("AlphaMaskCutoff")?) / 255.0,
        ..LegacyMaterial::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::TextureRef;
    use utils::AssetId;
    use uuid::Uuid;

    fn legacy_fields(mode: i32, cutoff: i32) -> llsd::Map {
        let mut fields: llsd::Map = [
            ("NormMap".into(), llsd::Value::Uuid(Uuid::from_u128(7))),
            ("SpecMap".into(), llsd::Value::Uuid(Uuid::nil())),
            ("SpecColor".into(), llsd::Value::Array([51, 102, 153, 204].map(llsd::Value::Integer).into())),
        ]
        .into();
        for (name, value) in [
            ("NormOffsetX", 2500),
            ("NormOffsetY", -5000),
            ("NormRepeatX", 20000),
            ("NormRepeatY", 30000),
            ("NormRotation", 15000),
            ("SpecOffsetX", -1250),
            ("SpecOffsetY", 7500),
            ("SpecRepeatX", -10000),
            ("SpecRepeatY", 0),
            ("SpecRotation", -20000),
            ("SpecExp", 137),
            ("EnvIntensity", 73),
            ("DiffuseAlphaMode", mode),
            ("AlphaMaskCutoff", cutoff),
        ] {
            fields.insert(name.into(), llsd::Value::Integer(value));
        }
        fields
    }

    fn legacy_bytes(mode: i32, cutoff: i32) -> Vec<u8> {
        binary::to_vec(&llsd::Value::Map(legacy_fields(mode, cutoff))).unwrap()
    }

    fn legacy_expected(alpha_mode: LegacyAlphaMode) -> LegacyMaterial {
        LegacyMaterial {
            normal: MaterialTexture {
                texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(7))),
                transform: TextureTransform { offset: [0.25, -0.5], scale: [2.0, 3.0], rotation: 1.5 },
                ..MaterialTexture::default()
            },
            specular: MaterialTexture {
                texture: TextureRef::Clear,
                transform: TextureTransform { offset: [-0.125, 0.75], scale: [-1.0, 0.0], rotation: -2.0 },
                ..MaterialTexture::default()
            },
            specular_color: [51, 102, 153, 204],
            specular_exponent: 137,
            environment_intensity: 73,
            alpha_mode,
            alpha_cutoff: 128.0 / 255.0,
            ..LegacyMaterial::default()
        }
    }

    #[test]
    fn legacy_channels_survive() {
        for (wire, mode) in
            [(0, LegacyAlphaMode::Opaque), (1, LegacyAlphaMode::Blend), (2, LegacyAlphaMode::Mask), (3, LegacyAlphaMode::Emissive)]
        {
            assert_eq!(decode(&legacy_bytes(wire, 128)), Ok(MaterialDescription::Legacy(legacy_expected(mode))));
        }
        assert_eq!(decode(&legacy_bytes(4, 0)), Err(Failure::Unsupported));
        assert_eq!(decode(&legacy_bytes(1, 256)), Err(Failure::Malformed));
        assert_eq!(decode(b"{\0\0\0\0}"), Err(Failure::Malformed));
    }

    #[test]
    fn legacy_fields_validate() {
        let fields = legacy_fields(1, 128);
        for name in fields.keys() {
            let mut missing = fields.clone();
            missing.remove(name);
            assert_eq!(decode(&binary::to_vec(&llsd::Value::Map(missing)).unwrap()), Err(Failure::Malformed), "missing {name}");
            let mut mistyped = fields.clone();
            mistyped.insert(name.clone(), llsd::Value::String("invalid".into()));
            assert_eq!(decode(&binary::to_vec(&llsd::Value::Map(mistyped)).unwrap()), Err(Failure::Malformed), "mistyped {name}");
        }
        for (name, value) in [
            ("SpecExp", llsd::Value::Integer(-1)),
            ("EnvIntensity", llsd::Value::Integer(256)),
            ("SpecColor", llsd::Value::Array(vec![llsd::Value::Integer(255); 3])),
            ("SpecColor", llsd::Value::Array(vec![llsd::Value::Integer(256); 4])),
        ] {
            let mut invalid = fields.clone();
            invalid.insert(name.into(), value);
            assert_eq!(decode(&binary::to_vec(&llsd::Value::Map(invalid)).unwrap()), Err(Failure::Malformed), "invalid {name}");
        }
    }
}
