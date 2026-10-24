use assets::{MaterialTexture, PbrAlphaMode, PbrMaterial, TextureRef};
use world::{FieldUpdate, PbrMaterialOverride, PbrTextureOverride};

pub(super) fn apply_overrides(mut material: PbrMaterial, overrides: &PbrMaterialOverride) -> PbrMaterial {
    for (texture, patch) in [
        (&mut material.base_color, overrides.base_color),
        (&mut material.normal, overrides.normal),
        (&mut material.metallic_roughness, overrides.metallic_roughness),
        (&mut material.emissive, overrides.emissive),
    ] {
        apply_texture(texture, patch);
    }
    if let Some(value) = overrides.base_color_factor {
        material.base_color_factor = value;
    }
    if let Some(value) = overrides.emissive_factor {
        material.emissive_factor = value;
    }
    if let Some(value) = overrides.metallic {
        material.metallic = value;
    }
    if let Some(value) = overrides.roughness {
        material.roughness = value;
    }
    if let Some(value) = overrides.alpha_mode {
        material.alpha_mode = match value {
            world::PbrAlphaMode::Opaque => PbrAlphaMode::Opaque,
            world::PbrAlphaMode::Mask => PbrAlphaMode::Mask,
            world::PbrAlphaMode::Blend => PbrAlphaMode::Blend,
        };
    }
    if let Some(value) = overrides.alpha_cutoff {
        material.alpha_cutoff = value;
    }
    if let Some(value) = overrides.double_sided {
        material.double_sided = value;
    }
    material
}

fn apply_texture(info: &mut MaterialTexture, patch: PbrTextureOverride) {
    match patch.texture {
        FieldUpdate::Keep => {}
        FieldUpdate::Clear => info.source = TextureRef::Clear,
        FieldUpdate::Set(id) => info.source = TextureRef::Asset(id),
    }
    if let Some(offset) = patch.transform.offset {
        info.transform.offset = offset;
    }
    if let Some(scale) = patch.transform.scale {
        info.transform.scale = scale;
    }
    if let Some(rotation) = patch.transform.rotation {
        info.transform.rotation = rotation;
    }
}

#[cfg(test)]
mod tests {
    use math::UvTransform;
    use utils::AssetId;
    use uuid::Uuid;
    use world::PbrTextureTransformOverride;

    use super::*;

    #[test]
    fn overrides_do_not_mutate_base() {
        let texture = MaterialTexture {
            source: TextureRef::Asset(AssetId::new(Uuid::from_u128(1))),
            transform: UvTransform { offset: [0.4, 0.8], scale: [2.0, 3.0], rotation: 1.0 },
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
            alpha_mode: PbrAlphaMode::Blend,
            alpha_cutoff: 0.2,
            double_sided: true,
            ..PbrMaterial::default()
        };
        let transform = PbrTextureTransformOverride { offset: Some([0.0; 2]), scale: Some([1.0; 2]), rotation: Some(0.0) };
        let overrides = PbrMaterialOverride {
            base_color: PbrTextureOverride { texture: FieldUpdate::Keep, transform },
            normal: PbrTextureOverride {
                texture: FieldUpdate::Clear,
                transform: PbrTextureTransformOverride { offset: Some([-1.0, 2.0]), ..Default::default() },
            },
            metallic_roughness: PbrTextureOverride {
                texture: FieldUpdate::Set(AssetId::new(Uuid::from_u128(3))),
                transform: PbrTextureTransformOverride { scale: Some([3.0, 4.0]), ..Default::default() },
            },
            emissive: PbrTextureOverride {
                texture: FieldUpdate::Keep,
                transform: PbrTextureTransformOverride { rotation: Some(-0.5), ..Default::default() },
            },
            base_color_factor: Some([1.0; 4]),
            emissive_factor: Some([0.0; 3]),
            metallic: Some(1.0),
            roughness: Some(1.0),
            alpha_mode: Some(world::PbrAlphaMode::Opaque),
            alpha_cutoff: Some(0.5),
            double_sided: Some(false),
        };
        let changed = apply_overrides(base, &overrides);
        assert_eq!(changed.base_color_factor, [1.0; 4]);
        assert_eq!(changed.emissive_factor, [0.0; 3]);
        assert_eq!(changed.metallic, 1.0);
        assert_eq!(changed.roughness, 1.0);
        assert_eq!(changed.alpha_mode, PbrAlphaMode::Opaque);
        assert_eq!(changed.alpha_cutoff, 0.5);
        assert!(!changed.double_sided);
        assert_eq!(changed.base_color.source, texture.source, "an inherited assignment keeps the base texture");
        assert_eq!(changed.normal.source, TextureRef::Clear);
        assert_eq!(changed.metallic_roughness.source, TextureRef::Asset(AssetId::new(Uuid::from_u128(3))));
        assert_eq!(changed.emissive.source, texture.source);
        assert_eq!(changed.base_color.transform, UvTransform::default());
        assert_eq!(changed.normal.transform, UvTransform { offset: [-1.0, 2.0], ..texture.transform });
        assert_eq!(changed.metallic_roughness.transform, UvTransform { scale: [3.0, 4.0], ..texture.transform });
        assert_eq!(changed.emissive.transform, UvTransform { rotation: -0.5, ..texture.transform });
        assert_eq!(changed.occlusion, texture, "independent occlusion data is retained");
        assert_eq!(changed.normal_scale, 0.8);
        assert_eq!(changed.occlusion_strength, 0.6);
        assert_eq!(apply_overrides(base, &PbrMaterialOverride::default()), base);
        assert_eq!(base.base_color_factor, [0.2; 4]);
        assert_eq!(base.normal.source, texture.source);
        assert_eq!(base.base_color.transform, texture.transform);
    }
}
