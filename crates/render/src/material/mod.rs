mod gpu;
mod legacy;
mod pbr;

pub(crate) use gpu::FaceParams;

use assets::{Failure, LegacyMaterial, MaterialDescription, PbrAlphaMode, PbrMaterial, TextureHandle};
use utils::AssetId;
use world::{FaceAppearance, PbrMaterialOverride};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaBehavior {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetState {
    pub asset: AssetId,
    pub resolution: Resolution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureBinding {
    state: AssetState,
    image: Option<TextureHandle>,
}

impl TextureBinding {
    pub fn state(&self) -> AssetState {
        self.state
    }

    pub fn image(&self) -> Option<&TextureHandle> {
        self.image.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FaceMaterial {
    pub slot: u8,
    pub material: MaterialDescription,
    pub legacy_state: Option<AssetState>,
    pub pbr_state: Option<AssetState>,
    pub alpha: AlphaBehavior,
    pub base_color_texture: Option<TextureBinding>,
}

impl FaceMaterial {
    pub(crate) fn resolve_base_color(&mut self, state: AssetState, image: Option<TextureHandle>) {
        if self.legacy_state.is_none()
            && self.alpha == AlphaBehavior::Unknown
            && let Some(info) = image.as_ref().and_then(TextureHandle::info)
        {
            self.alpha = match info.format.alpha() {
                assets::PixelAlpha::Opaque => AlphaBehavior::Opaque,
                assets::PixelAlpha::Straight => AlphaBehavior::Blend,
            };
        }
        self.base_color_texture = Some(TextureBinding { state, image });
    }

    pub fn base_color_factor(&self) -> [f32; 4] {
        match self.material {
            MaterialDescription::Legacy(material) => material.diffuse_color,
            MaterialDescription::Pbr(material) => material.base_color_factor,
        }
    }

    pub fn double_sided(&self) -> bool {
        matches!(self.material, MaterialDescription::Pbr(material) if material.double_sided)
    }
}

pub(crate) fn resolve(
    authored: FaceAppearance,
    legacy: Option<LegacyMaterial>,
    pbr: Option<PbrMaterial>,
    overrides: Option<&PbrMaterialOverride>,
) -> (MaterialDescription, AlphaBehavior) {
    match pbr {
        Some(material) => {
            let material = overrides.map_or(material, |overrides| pbr::apply_overrides(material, overrides));
            let alpha = match material.alpha_mode {
                PbrAlphaMode::Opaque => AlphaBehavior::Opaque,
                PbrAlphaMode::Mask => AlphaBehavior::Mask(material.alpha_cutoff),
                PbrAlphaMode::Blend => AlphaBehavior::Blend,
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
    use assets::{LegacyAlphaMode, MaterialTexture, TextureRef};
    use uuid::Uuid;
    use world::PbrAlphaMode;

    use super::*;

    #[test]
    fn pbr_precedes_complete_legacy() {
        let authored =
            FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(1))), color: [255, 0, 0, 128], ..FaceAppearance::default() };
        let legacy = LegacyMaterial {
            normal: MaterialTexture { source: TextureRef::from(Uuid::from_u128(2)), ..MaterialTexture::default() },
            alpha_mode: LegacyAlphaMode::Opaque,
            ..LegacyMaterial::default()
        };
        let pbr = PbrMaterial { base_color_factor: [0.4; 4], ..PbrMaterial::default() };
        let overrides = PbrMaterialOverride { alpha_mode: Some(PbrAlphaMode::Mask), alpha_cutoff: Some(0.7), ..Default::default() };

        let (material, alpha) = resolve(authored, Some(legacy), Some(pbr), Some(&overrides));
        let MaterialDescription::Pbr(material) = material else { panic!("available PBR must take precedence over legacy") };
        assert_eq!(material.base_color_factor, [0.4; 4], "legacy tint must not modify PBR");
        assert_eq!(alpha, AlphaBehavior::Mask(0.7), "alpha classification must follow the applied PBR overrides");

        let (fallback, alpha) = resolve(authored, Some(legacy), None, Some(&overrides));
        let MaterialDescription::Legacy(fallback) = fallback else { panic!("unavailable PBR must use legacy") };
        assert_eq!(fallback.diffuse_color, [1.0, 0.0, 0.0, 128.0 / 255.0]);
        assert_eq!(fallback.diffuse.source, TextureRef::from(Uuid::from_u128(1)));
        assert_eq!(fallback.normal, legacy.normal, "fallback retains fetched legacy properties");
        assert_eq!(alpha, AlphaBehavior::Blend, "PBR overrides must not alter legacy fallback");
    }
}
