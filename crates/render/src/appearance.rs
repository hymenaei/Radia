use assets::{MaterialDescription, Sampler, TexGen, TextureTransform};
use scene::{Alpha, FaceMaterial};

use crate::texture::TextureKey;

#[cfg(test)]
#[path = "appearance_tests.rs"]
mod tests;

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
        result.color = face.base_color();
        result.double_sided = face.double_sided();
        match face.alpha {
            Alpha::Mask(cutoff) => {
                result.params[1] = 1.0;
                result.params[2] = cutoff;
            }
            Alpha::Blend => {
                result.params[1] = 2.0;
                result.blend = true;
            }
            Alpha::Opaque | Alpha::Unknown => {}
        }
        let (matrix, sampler, supported) = match face.material {
            MaterialDescription::Legacy(material) => {
                result.params[0] = f32::from(u8::from(material.fullbright));
                result.params[3] = f32::from(u8::from(material.texgen == TexGen::Planar));
                (legacy_uv(material.diffuse.transform), Sampler::default(), !matches!(material.texgen, TexGen::Unsupported(_)))
            }
            MaterialDescription::Pbr(material) => {
                result.params[0] = f32::from(u8::from(material.unlit));
                (pbr_uv(material.base_color.transform), material.base_color.sampler, material.base_color.tex_coord == 0)
            }
        };
        if supported && matrix.iter().flatten().all(|value| value.is_finite()) {
            result.uv = matrix;
            if let Some(texture) = &face.base_color {
                result.texture = Some(TextureKey { id: texture.id(), sampler });
            }
        }
        result
    }
}

fn legacy_uv(transform: TextureTransform) -> [[f32; 4]; 2] {
    let (s, c) = f64::from(transform.rotation).sin_cos();
    let [x, y] = transform.scale.map(f64::from);
    let [u, v] = transform.offset.map(f64::from);
    [
        [x * c, x * s, u + 0.5 - 0.5 * x * (c + s), 0.0].map(|n| n as f32),
        [y * s, -y * c, 0.5 - v + 0.5 * y * (c - s), 0.0].map(|n| n as f32),
    ]
}

fn pbr_uv(transform: TextureTransform) -> [[f32; 4]; 2] {
    let (s, c) = f64::from(transform.rotation).sin_cos();
    let [x, y] = transform.scale.map(f64::from);
    let [u, v] = transform.offset.map(f64::from);
    [[c * x, s * y, u - s * y, 0.0].map(|n| n as f32), [s * x, -c * y, v + c * y, 0.0].map(|n| n as f32)]
}
