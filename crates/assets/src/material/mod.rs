mod json;
pub(crate) mod legacy;
pub(crate) mod pbr;

pub use legacy::{LegacyAlphaMode, LegacyMaterial, TexGen};
pub use pbr::{PbrAlphaMode, PbrMaterial};

use utils::AssetId;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Filter {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MinFilter {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapNearest,
    NearestMipmapLinear,
    LinearMipmapLinear,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Wrap {
    ClampToEdge,
    MirroredRepeat,
    #[default]
    Repeat,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Sampler {
    pub mag_filter: Option<Filter>,
    pub min_filter: Option<MinFilter>,
    pub wrap_s: Wrap,
    pub wrap_t: Wrap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureRef {
    Absent,
    Clear,
    Asset(AssetId),
}

impl From<Uuid> for TextureRef {
    fn from(id: Uuid) -> Self {
        if id.is_nil() || id == Uuid::max() { Self::Clear } else { Self::Asset(AssetId::new(id)) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialTexture {
    pub texture: TextureRef,
    pub transform: TextureTransform,
    pub tex_coord: u32,
    pub sampler: Sampler,
}

impl Default for MaterialTexture {
    fn default() -> Self {
        Self { texture: TextureRef::Absent, transform: TextureTransform::default(), tex_coord: 0, sampler: Sampler::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
}

impl Default for TextureTransform {
    fn default() -> Self {
        Self { offset: [0.0; 2], scale: [1.0; 2], rotation: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaterialDescription {
    Legacy(LegacyMaterial),
    Pbr(PbrMaterial),
}

impl MaterialDescription {
    pub fn base_color_texture(&self) -> &MaterialTexture {
        match self {
            Self::Legacy(material) => &material.diffuse,
            Self::Pbr(material) => &material.base_color,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_texture_references_are_canonical() {
        assert_eq!(TextureRef::from(Uuid::nil()), TextureRef::Clear);
        assert_eq!(TextureRef::from(Uuid::max()), TextureRef::Clear);
        let id = Uuid::from_u128(7);
        assert_eq!(TextureRef::from(id), TextureRef::Asset(AssetId::new(id)));
    }

    #[test]
    fn color_channel_selection() {
        let color = MaterialTexture {
            texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(17))),
            transform: TextureTransform { offset: [0.25, -0.5], scale: [2.0, -3.0], rotation: 0.75 },
            tex_coord: 2,
            sampler: Sampler { wrap_s: Wrap::ClampToEdge, ..Sampler::default() },
        };
        let legacy = MaterialDescription::Legacy(LegacyMaterial { diffuse: color, ..LegacyMaterial::default() });
        let pbr = MaterialDescription::Pbr(PbrMaterial { base_color: color, ..PbrMaterial::default() });
        for material in [legacy, pbr] {
            assert_eq!(*material.base_color_texture(), color, "the color channel retains its own reference, UVs and sampler");
        }
    }
}
