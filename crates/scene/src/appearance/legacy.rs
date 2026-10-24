use assets::{LegacyAlphaMode, LegacyMaterial, MaterialTexture, TexGen, TextureRef, TextureTransform};
use math::srgb8_to_linear;
use world::FaceAppearance;

use super::Alpha;

pub(super) fn resolve(source: FaceAppearance, base: Option<LegacyMaterial>) -> (LegacyMaterial, Alpha) {
    let texture = TextureRef::from(source.texture_id);
    let alpha = if source.color[3] < u8::MAX {
        Alpha::Blend
    } else if let Some(base) = base {
        match base.alpha_mode {
            LegacyAlphaMode::Opaque | LegacyAlphaMode::Emissive => Alpha::Opaque,
            LegacyAlphaMode::Blend => Alpha::Blend,
            LegacyAlphaMode::Mask => Alpha::Mask(base.alpha_cutoff),
        }
    } else if !source.material_id.is_nil() || matches!(texture, TextureRef::Asset(_)) {
        Alpha::Unknown
    } else {
        Alpha::Opaque
    };
    let material = LegacyMaterial {
        diffuse_color: [
            srgb8_to_linear(source.color[0]),
            srgb8_to_linear(source.color[1]),
            srgb8_to_linear(source.color[2]),
            f32::from(source.color[3]) / 255.0,
        ],
        diffuse: MaterialTexture {
            texture,
            transform: TextureTransform {
                offset: [source.offset_s, source.offset_t],
                scale: [source.scale_s, source.scale_t],
                rotation: source.rotation,
            },
            ..MaterialTexture::default()
        },
        texgen: match (source.media_texgen >> 1) & 3 {
            0 => TexGen::Default,
            1 => TexGen::Planar,
            value => TexGen::Unsupported(value),
        },
        fullbright: source.bump_shiny_fullbright & 0x20 != 0,
        bump: source.bump_shiny_fullbright & 0x1f,
        shiny: source.bump_shiny_fullbright >> 6,
        glow: source.glow,
        ..base.unwrap_or_default()
    };
    (material, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use utils::AssetId;
    use uuid::Uuid;

    #[test]
    fn legacy_combines_diffuse_and_maps() {
        let source = FaceAppearance {
            texture_id: Uuid::from_u128(1),
            color: [128, 0, 255, 128],
            scale_s: -2.0,
            scale_t: 3.0,
            offset_s: 0.25,
            offset_t: -0.5,
            rotation: 1.5,
            bump_shiny_fullbright: 0xe5,
            media_texgen: 3,
            glow: 0.4,
            ..FaceAppearance::default()
        };
        let maps = LegacyMaterial {
            normal: MaterialTexture { texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(2))), ..MaterialTexture::default() },
            specular: MaterialTexture { texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(3))), ..MaterialTexture::default() },
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
        assert_eq!(material.diffuse.texture, TextureRef::Asset(AssetId::new(Uuid::from_u128(1))));
        assert_eq!(material.diffuse.transform, TextureTransform { offset: [0.25, -0.5], scale: [-2.0, 3.0], rotation: 1.5 });
        assert_eq!(material.texgen, TexGen::Planar);
        assert!(material.fullbright);
        assert_eq!(material.bump, 5);
        assert_eq!(material.shiny, 3);
        assert_eq!(material.glow, 0.4);
        assert_eq!(material.normal, maps.normal);
        assert_eq!(material.specular, maps.specular);
        assert_eq!(material.specular_color, [10, 20, 30, 255]);
        assert_eq!(material.specular_exponent, 71);
        assert_eq!(alpha, Alpha::Blend, "TextureEntry transparency also applies to masked legacy faces");
    }

    #[test]
    fn unresolved_alpha_stays_unknown() {
        for texture_id in [Uuid::nil(), Uuid::max()] {
            let source = FaceAppearance { texture_id, ..FaceAppearance::default() };
            assert_eq!(resolve(source, None).1, Alpha::Opaque, "a clear texture has no unknown alpha");
        }
        let source = FaceAppearance { texture_id: Uuid::from_u128(1), ..FaceAppearance::default() };
        assert_eq!(resolve(source, None).1, Alpha::Unknown);
        let missing = FaceAppearance { material_id: Uuid::from_u128(2), ..FaceAppearance::default() };
        assert_eq!(resolve(missing, None).1, Alpha::Unknown);
        for (mode, expected) in [
            (LegacyAlphaMode::Opaque, Alpha::Opaque),
            (LegacyAlphaMode::Blend, Alpha::Blend),
            (LegacyAlphaMode::Mask, Alpha::Mask(0.4)),
            (LegacyAlphaMode::Emissive, Alpha::Opaque),
        ] {
            let maps = LegacyMaterial { alpha_mode: mode, alpha_cutoff: 0.4, ..LegacyMaterial::default() };
            assert_eq!(resolve(source, Some(maps)).1, expected);
        }
    }
}
