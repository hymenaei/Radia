use assets::{MaterialTexture, PbrAlphaMode, PbrMaterial, TextureRef};
use uuid::Uuid;
use world::{GltfAlphaMode, GltfMaterialOverride, GltfTextureTransform};

pub(super) fn apply_overrides(mut material: PbrMaterial, overrides: &GltfMaterialOverride) -> PbrMaterial {
    for (index, info) in
        [&mut material.base_color, &mut material.normal, &mut material.metallic_roughness, &mut material.emissive].into_iter().enumerate()
    {
        apply_texture(info, overrides.textures[index], overrides.texture_transforms[index]);
    }
    if let Some(value) = overrides.base_color {
        material.base_color_factor = value;
    }
    if let Some(value) = overrides.emissive {
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
            GltfAlphaMode::Opaque => PbrAlphaMode::Opaque,
            GltfAlphaMode::Mask => PbrAlphaMode::Mask,
            GltfAlphaMode::Blend => PbrAlphaMode::Blend,
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

fn apply_texture(info: &mut MaterialTexture, texture: Option<Uuid>, transform: GltfTextureTransform) {
    if let Some(id) = texture.filter(|id| !id.is_nil()) {
        info.texture = TextureRef::from(id);
    }
    if let Some(offset) = transform.offset {
        info.transform.offset = offset;
    }
    if let Some(scale) = transform.scale {
        info.transform.scale = scale;
    }
    if let Some(rotation) = transform.rotation {
        info.transform.rotation = rotation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assets::{TextureRef, TextureTransform};
    use utils::AssetId;

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
            alpha_mode: PbrAlphaMode::Blend,
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
        assert_eq!(changed.alpha_mode, PbrAlphaMode::Opaque);
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
}
