use assets::{LegacyAlphaMode, LegacyMaterial, MaterialTexture, TextureMapping, TextureRef};
use math::srgb8_to_linear;
use world::FaceAppearance;

use super::AlphaBehavior;

pub(super) fn resolve(source: FaceAppearance, base: Option<LegacyMaterial>) -> (LegacyMaterial, AlphaBehavior) {
    let texture = source.texture.map_or(TextureRef::Clear, TextureRef::Asset);
    let alpha = if source.color[3] < u8::MAX {
        AlphaBehavior::Blend
    } else if let Some(base) = base {
        match base.alpha_mode {
            LegacyAlphaMode::Opaque | LegacyAlphaMode::Emissive => AlphaBehavior::Opaque,
            LegacyAlphaMode::Blend => AlphaBehavior::Blend,
            LegacyAlphaMode::Mask => AlphaBehavior::Mask(base.alpha_cutoff),
        }
    } else if source.material.is_some() || matches!(texture, TextureRef::Asset(_)) {
        AlphaBehavior::Unknown
    } else {
        AlphaBehavior::Opaque
    };
    let material = LegacyMaterial {
        diffuse_color: [
            srgb8_to_linear(source.color[0]),
            srgb8_to_linear(source.color[1]),
            srgb8_to_linear(source.color[2]),
            f32::from(source.color[3]) / 255.0,
        ],
        diffuse: MaterialTexture { source: texture, transform: source.transform, ..MaterialTexture::default() },
        mapping: match source.mapping {
            world::TextureMapping::Default => TextureMapping::Default,
            world::TextureMapping::Planar => TextureMapping::Planar,
            world::TextureMapping::Unsupported(value) => TextureMapping::Unsupported(value),
        },
        fullbright: source.fullbright,
        bump: source.bump,
        shiny: source.shiny,
        glow: source.glow,
        ..base.unwrap_or_default()
    };
    (material, alpha)
}

#[cfg(test)]
mod tests {
    use math::UvTransform;
    use utils::AssetId;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn legacy_combines_diffuse_and_maps() {
        let source = FaceAppearance {
            texture: Some(AssetId::new(Uuid::from_u128(1))),
            color: [128, 0, 255, 128],
            transform: UvTransform { offset: [0.25, -0.5], scale: [-2.0, 3.0], rotation: 1.5 },
            bump: 5,
            shiny: 3,
            fullbright: true,
            mapping: world::TextureMapping::Planar,
            media: true,
            glow: 0.4,
            ..FaceAppearance::default()
        };
        let maps = LegacyMaterial {
            normal: MaterialTexture { source: TextureRef::Asset(AssetId::new(Uuid::from_u128(2))), ..MaterialTexture::default() },
            specular: MaterialTexture { source: TextureRef::Asset(AssetId::new(Uuid::from_u128(3))), ..MaterialTexture::default() },
            specular_color: [10, 20, 30, 255],
            specular_exponent: 71,
            alpha_mode: LegacyAlphaMode::Mask,
            alpha_cutoff: 0.25,
            ..LegacyMaterial::default()
        };
        let (material, alpha) = resolve(source, Some(maps));
        assert!((material.diffuse_color[0] - 0.2158605).abs() < 1e-6);
        assert_eq!(&material.diffuse_color[1..3], &[0.0, 1.0]);
        assert_eq!(material.diffuse_color[3], 128.0 / 255.0);
        assert_eq!(material.diffuse.source, TextureRef::Asset(AssetId::new(Uuid::from_u128(1))));
        assert_eq!(material.diffuse.transform, UvTransform { offset: [0.25, -0.5], scale: [-2.0, 3.0], rotation: 1.5 });
        assert_eq!(material.mapping, TextureMapping::Planar);
        assert!(material.fullbright);
        assert_eq!(material.bump, 5);
        assert_eq!(material.shiny, 3);
        assert_eq!(material.glow, 0.4);
        assert_eq!(material.normal, maps.normal);
        assert_eq!(material.specular, maps.specular);
        assert_eq!(material.specular_color, [10, 20, 30, 255]);
        assert_eq!(material.specular_exponent, 71);
        assert_eq!(alpha, AlphaBehavior::Blend, "TextureEntry transparency also applies to masked legacy faces");
    }

    #[test]
    fn unresolved_alpha_stays_unknown() {
        assert_eq!(resolve(FaceAppearance::default(), None).1, AlphaBehavior::Opaque, "a clear texture has no unknown alpha");
        let source = FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(1))), ..FaceAppearance::default() };
        assert_eq!(resolve(source, None).1, AlphaBehavior::Unknown);
        let missing = FaceAppearance { material: Some(AssetId::new(Uuid::from_u128(2))), ..FaceAppearance::default() };
        assert_eq!(resolve(missing, None).1, AlphaBehavior::Unknown);
        for (mode, expected) in [
            (LegacyAlphaMode::Opaque, AlphaBehavior::Opaque),
            (LegacyAlphaMode::Blend, AlphaBehavior::Blend),
            (LegacyAlphaMode::Mask, AlphaBehavior::Mask(0.4)),
            (LegacyAlphaMode::Emissive, AlphaBehavior::Opaque),
        ] {
            let maps = LegacyMaterial { alpha_mode: mode, alpha_cutoff: 0.4, ..LegacyMaterial::default() };
            assert_eq!(resolve(source, Some(maps)).1, expected);
        }
    }
}
