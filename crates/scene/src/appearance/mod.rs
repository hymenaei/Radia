mod legacy;
mod pbr;

use assets::{Failure, LegacyMaterial, MaterialDescription, PbrAlphaMode, PbrMaterial, TextureHandle, TextureRef};
use utils::AssetId;
use world::{FaceAppearance, GltfMaterialOverride};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alpha {
    Opaque,
    Mask(f32),
    Blend,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Pending,
    Ready,
    Failed(Failure),
    Capacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetState {
    pub asset: AssetId,
    pub resolution: Resolution,
}

#[derive(Clone, Debug)]
pub struct FaceMaterial {
    pub slot: u8,
    pub material: MaterialDescription,
    pub legacy_state: Option<AssetState>,
    pub pbr_state: Option<AssetState>,
    pub alpha: Alpha,
    pub base_color: Option<TextureHandle>,
    pub base_color_state: Option<AssetState>,
}

impl FaceMaterial {
    pub(crate) fn resolve_base_color(&mut self, image: TextureHandle) {
        if self.legacy_state.is_none()
            && self.alpha == Alpha::Unknown
            && let Some(info) = image.info()
        {
            self.alpha = match info.format.alpha() {
                assets::Alpha::Opaque => Alpha::Opaque,
                assets::Alpha::Straight => Alpha::Blend,
            };
        }
        self.base_color = Some(image);
    }

    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        self.slot == other.slot
            && self.material == other.material
            && self.alpha == other.alpha
            && self.legacy_state == other.legacy_state
            && self.pbr_state == other.pbr_state
            && self.base_color_state == other.base_color_state
            && self.base_color.as_ref().map(|image| (image.id(), image.info()))
                == other.base_color.as_ref().map(|image| (image.id(), image.info()))
    }
    pub fn base_color(&self) -> [f32; 4] {
        match self.material {
            MaterialDescription::Legacy(material) => material.diffuse_color,
            MaterialDescription::Pbr(material) => material.base_color_factor,
        }
    }

    pub fn double_sided(&self) -> bool {
        matches!(self.material, MaterialDescription::Pbr(material) if material.double_sided)
    }

    pub fn base_color_texture(&self) -> TextureRef {
        self.material.base_color_texture().texture
    }
}

pub(crate) fn resolve(
    authored: FaceAppearance,
    legacy: Option<LegacyMaterial>,
    pbr: Option<PbrMaterial>,
    overrides: Option<&GltfMaterialOverride>,
) -> (MaterialDescription, Alpha) {
    match pbr {
        Some(material) => {
            let material = overrides.map_or(material, |overrides| pbr::apply_overrides(material, overrides));
            let alpha = match material.alpha_mode {
                PbrAlphaMode::Opaque => Alpha::Opaque,
                PbrAlphaMode::Mask => Alpha::Mask(material.alpha_cutoff),
                PbrAlphaMode::Blend => Alpha::Blend,
            };
            (MaterialDescription::Pbr(material), alpha)
        }
        None => {
            let (material, alpha) = legacy::resolve(authored, legacy);
            (MaterialDescription::Legacy(material), alpha)
        }
    }
}

#[cfg(test)]
mod tests {
    use assets::{LegacyAlphaMode, MaterialTexture};
    use uuid::Uuid;
    use world::GltfAlphaMode;

    use super::*;

    #[test]
    fn pbr_precedes_complete_legacy() {
        let authored = FaceAppearance { texture_id: Uuid::from_u128(1), color: [255, 0, 0, 128], ..FaceAppearance::default() };
        let legacy = LegacyMaterial {
            normal: MaterialTexture { texture: TextureRef::from(Uuid::from_u128(2)), ..MaterialTexture::default() },
            alpha_mode: LegacyAlphaMode::Opaque,
            ..LegacyMaterial::default()
        };
        let pbr = PbrMaterial { base_color_factor: [0.4; 4], ..PbrMaterial::default() };
        let overrides = GltfMaterialOverride { alpha_mode: Some(GltfAlphaMode::Mask), alpha_cutoff: Some(0.7), ..Default::default() };

        let (material, alpha) = resolve(authored, Some(legacy), Some(pbr), Some(&overrides));
        let MaterialDescription::Pbr(material) = material else { panic!("available PBR must take precedence over legacy") };
        assert_eq!(material.base_color_factor, [0.4; 4], "legacy tint must not modify PBR");
        assert_eq!(alpha, Alpha::Mask(0.7), "alpha classification must follow the applied PBR overrides");

        let (fallback, alpha) = resolve(authored, Some(legacy), None, Some(&overrides));
        let MaterialDescription::Legacy(fallback) = fallback else { panic!("unavailable PBR must use legacy") };
        assert_eq!(fallback.diffuse_color, [1.0, 0.0, 0.0, 128.0 / 255.0]);
        assert_eq!(fallback.diffuse.texture, TextureRef::from(Uuid::from_u128(1)));
        assert_eq!(fallback.normal, legacy.normal, "fallback retains fetched legacy properties");
        assert_eq!(alpha, Alpha::Blend, "PBR overrides must not alter legacy fallback");
    }
}
