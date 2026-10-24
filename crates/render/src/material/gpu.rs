use assets::{MaterialDescription, Sampler, TextureMapping};
use math::UvTransform;

use super::{AlphaBehavior, FaceMaterial};
use crate::texture::TextureKey;

pub(crate) struct FaceParams {
    pub(crate) color: [f32; 4],
    pub(crate) uv: [[f32; 4]; 2],
    pub(crate) params: [f32; 4],
    pub(crate) texture: Option<TextureKey>,
    pub(crate) blend: bool,
    pub(crate) double_sided: bool,
}

impl FaceParams {
    pub(crate) fn new(face: Option<&FaceMaterial>) -> Self {
        let mut result = Self {
            color: [0.38, 0.38, 0.38, 1.0],
            uv: [[1.0, 0.0, 0.0, 0.0], [0.0, -1.0, 1.0, 0.0]],
            params: [0.0; 4],
            texture: None,
            blend: false,
            double_sided: false,
        };
        let Some(face) = face else { return result };
        result.color = face.base_color_factor();
        result.double_sided = face.double_sided();
        match face.alpha {
            AlphaBehavior::Mask(cutoff) => {
                result.params[1] = 1.0;
                result.params[2] = cutoff;
            }
            AlphaBehavior::Blend => {
                result.params[1] = 2.0;
                result.blend = true;
            }
            AlphaBehavior::Opaque | AlphaBehavior::Unknown => {}
        }
        let (matrix, sampler, supported) = match face.material {
            MaterialDescription::Legacy(material) => {
                result.params[0] = f32::from(u8::from(material.fullbright));
                result.params[3] = f32::from(u8::from(material.mapping == TextureMapping::Planar));
                (legacy_uv(material.diffuse.transform), Sampler::default(), !matches!(material.mapping, TextureMapping::Unsupported(_)))
            }
            MaterialDescription::Pbr(material) => {
                result.params[0] = f32::from(u8::from(material.unlit));
                (pbr_uv(material.base_color.transform), material.base_color.sampler, material.base_color.tex_coord == 0)
            }
        };
        if supported && matrix.iter().flatten().all(|value| value.is_finite()) {
            result.uv = matrix;
            if let Some(texture) = face.base_color_texture.as_ref().and_then(|binding| binding.image()) {
                result.texture = Some(TextureKey { id: texture.id(), sampler });
            }
        }
        result
    }
}

fn legacy_uv(transform: UvTransform) -> [[f32; 4]; 2] {
    let (s, c) = f64::from(transform.rotation).sin_cos();
    let [x, y] = transform.scale.map(f64::from);
    let [u, v] = transform.offset.map(f64::from);
    [
        [x * c, x * s, u + 0.5 - 0.5 * x * (c + s), 0.0].map(|n| n as f32),
        [y * s, -y * c, 0.5 - v + 0.5 * y * (c - s), 0.0].map(|n| n as f32),
    ]
}

fn pbr_uv(transform: UvTransform) -> [[f32; 4]; 2] {
    let (s, c) = f64::from(transform.rotation).sin_cos();
    let [x, y] = transform.scale.map(f64::from);
    let [u, v] = transform.offset.map(f64::from);
    [[c * x, s * y, u - s * y, 0.0].map(|n| n as f32), [s * x, -c * y, v + c * y, 0.0].map(|n| n as f32)]
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use assets::{MaterialTexture, PbrMaterial};
    use utils::AssetId;
    use uuid::Uuid;

    use super::*;
    use crate::material::{AssetState, Resolution};

    fn mapped(matrix: [[f32; 4]; 2], uv: [f32; 2]) -> [f32; 2] {
        matrix.map(|row| row[0] * uv[0] + row[1] * uv[1] + row[2])
    }

    #[test]
    fn legacy_transform_order() {
        let matrix = legacy_uv(UvTransform { offset: [0.125, -0.25], scale: [2.0, 3.0], rotation: FRAC_PI_2 });
        let actual = mapped(matrix, [0.75, 0.625]);
        for (actual, expected) in actual.into_iter().zip([0.875, 1.5]) {
            assert!((actual - expected).abs() < 1e-6, "legacy UVs rotate about center before scaling, then flip V");
        }
        assert_eq!(mapped(legacy_uv(UvTransform::default()), [0.0, 0.0]), [0.0, 1.0]);
    }

    #[test]
    fn pbr_transform_order() {
        let matrix = pbr_uv(UvTransform { offset: [0.125, -0.25], scale: [2.0, 3.0], rotation: FRAC_PI_2 });
        let actual = mapped(matrix, [0.75, 0.625]);
        for (actual, expected) in actual.into_iter().zip([-1.0, 1.25]) {
            assert!((actual - expected).abs() < 1e-6, "glTF UVs flip V before scale, origin rotation, and translation");
        }
        assert_eq!(mapped(pbr_uv(UvTransform::default()), [0.0, 1.0]), [0.0, 0.0]);
    }

    #[test]
    fn fallback_preserves_material() {
        let id = AssetId::new(Uuid::from_u128(42));
        let mut material = PbrMaterial {
            base_color_factor: [0.2, 0.3, 0.4, 0.5],
            double_sided: true,
            unlit: true,
            base_color: MaterialTexture { tex_coord: 1, ..Default::default() },
            ..Default::default()
        };
        let mut face = FaceMaterial {
            slot: 0,
            material: MaterialDescription::Pbr(material),
            legacy_state: None,
            pbr_state: None,
            alpha: AlphaBehavior::Mask(0.7),
            base_color_texture: None,
        };
        face.resolve_base_color(AssetState { asset: id, resolution: Resolution::Pending }, None);
        let params = FaceParams::new(Some(&face));
        assert_eq!(params.color, material.base_color_factor);
        assert_eq!(params.params, [1.0, 1.0, 0.7, 0.0]);
        assert!(params.double_sided);
        assert!(!params.blend);
        assert_eq!(params.texture, None, "unsupported UV sets use a neutral image");
        material.base_color.tex_coord = 0;
        face.material = MaterialDescription::Pbr(material);
        face.alpha = AlphaBehavior::Blend;
        let params = FaceParams::new(Some(&face));
        assert_eq!(params.texture, None, "pending assets do not own a GPU texture");
        assert!(params.blend);
        assert_eq!(params.params[1], 2.0);
    }
}
