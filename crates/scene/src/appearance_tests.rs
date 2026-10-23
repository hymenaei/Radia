use super::*;
use assets::AlphaMode;

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
    let (material, alpha) = resolve_legacy(source, Some(maps), None);
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
    let source = FaceAppearance { texture_id: Uuid::from_u128(1), ..FaceAppearance::default() };
    assert_eq!(resolve_legacy(source, None, None).1, Alpha::Unknown);
    let missing = AssetState { asset: AssetId::new(Uuid::from_u128(2)), resolution: Resolution::Failed(Failure::NotFound) };
    assert_eq!(resolve_legacy(source, None, Some(missing)).1, Alpha::Unknown);
    for (mode, expected) in [
        (LegacyAlphaMode::Opaque, Alpha::Opaque),
        (LegacyAlphaMode::Blend, Alpha::Blend),
        (LegacyAlphaMode::Mask, Alpha::Mask(0.4)),
        (LegacyAlphaMode::Emissive, Alpha::Opaque),
    ] {
        let maps = LegacyMaterial { alpha_mode: mode, alpha_cutoff: 0.4, ..LegacyMaterial::default() };
        assert_eq!(resolve_legacy(source, Some(maps), None).1, expected);
    }
}

#[test]
fn overrides_do_not_mutate_base() {
    let texture = MaterialTexture {
        texture: TextureRef::Asset(AssetId::new(Uuid::from_u128(1))),
        transform: TextureTransform { offset: [0.4, 0.8], scale: [2.0, 3.0], rotation: 1.0 },
        ..MaterialTexture::default()
    };
    let base = PbrMaterial {
        base_color_factor: [0.2; 4],
        base_color: texture,
        normal: texture,
        metallic_roughness: texture,
        emissive: texture,
        occlusion: texture,
        metallic: 0.2,
        roughness: 0.3,
        emissive_factor: [0.4; 3],
        normal_scale: 0.8,
        occlusion_strength: 0.6,
        alpha_mode: AlphaMode::Blend,
        alpha_cutoff: 0.2,
        double_sided: true,
        ..PbrMaterial::default()
    };
    let overrides = GltfMaterialOverride {
        textures: [Some(Uuid::nil()), Some(Uuid::max()), Some(Uuid::from_u128(3)), None],
        base_color: Some([1.0; 4]),
        emissive: Some([0.0; 3]),
        metallic: Some(1.0),
        roughness: Some(1.0),
        alpha_mode: Some(GltfAlphaMode::Opaque),
        alpha_cutoff: Some(0.5),
        double_sided: Some(false),
        texture_transforms: [GltfTextureTransform { offset: Some([0.0; 2]), scale: Some([1.0; 2]), rotation: Some(0.0) }; 4],
    };
    let changed = apply_overrides(base, &overrides);
    assert_eq!(changed.base_color_factor, [1.0; 4]);
    assert_eq!(changed.emissive_factor, [0.0; 3]);
    assert_eq!(changed.metallic, 1.0);
    assert_eq!(changed.roughness, 1.0);
    assert_eq!(changed.alpha_mode, AlphaMode::Opaque);
    assert_eq!(changed.alpha_cutoff, 0.5);
    assert!(!changed.double_sided);
    assert_eq!(changed.base_color.texture, texture.texture, "nil override inherits the base texture");
    assert_eq!(changed.normal.texture, TextureRef::Clear);
    assert_eq!(changed.metallic_roughness.texture, TextureRef::Asset(AssetId::new(Uuid::from_u128(3))));
    assert_eq!(changed.emissive.texture, texture.texture);
    assert_eq!(changed.base_color.transform, TextureTransform::default());
    assert_eq!(changed.occlusion, texture, "independent occlusion data is retained");
    assert_eq!(changed.normal_scale, 0.8);
    assert_eq!(changed.occlusion_strength, 0.6);
    assert_eq!(apply_overrides(base, &GltfMaterialOverride::default()), base);
    assert_eq!(base.base_color_factor, [0.2; 4]);
    assert_eq!(base.normal.texture, texture.texture);
    assert_eq!(base.base_color.transform, texture.transform);
}
