use assets::{
    AlphaMode, Failure, LegacyAlphaMode, LegacyMaterial, MaterialDescription, MaterialTexture, PbrMaterial, TexGen, TextureHandle,
    TextureRef, TextureTransform,
};
use utils::AssetId;
use uuid::Uuid;
use world::{FaceAppearance, GltfAlphaMode, GltfMaterialOverride, GltfTextureTransform};

#[cfg(test)]
#[path = "appearance_tests.rs"]
mod tests;

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

pub(crate) fn resolve_legacy(source: FaceAppearance, maps: Option<LegacyMaterial>, state: Option<AssetState>) -> (LegacyMaterial, Alpha) {
    let alpha = if source.color[3] < u8::MAX {
        Alpha::Blend
    } else if let Some(maps) = maps {
        match maps.alpha_mode {
            LegacyAlphaMode::Opaque | LegacyAlphaMode::Emissive => Alpha::Opaque,
            LegacyAlphaMode::Blend => Alpha::Blend,
            LegacyAlphaMode::Mask => Alpha::Mask(maps.alpha_cutoff),
        }
    } else if state.is_some() || !source.texture_id.is_nil() {
        Alpha::Unknown
    } else {
        Alpha::Opaque
    };
    let material = LegacyMaterial {
        diffuse_color: [linear(source.color[0]), linear(source.color[1]), linear(source.color[2]), f32::from(source.color[3]) / 255.0],
        diffuse: MaterialTexture {
            texture: texture_ref(source.texture_id),
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
        ..maps.unwrap_or_default()
    };
    (material, alpha)
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

pub(crate) fn apply_overrides(mut material: PbrMaterial, overrides: &GltfMaterialOverride) -> PbrMaterial {
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
            GltfAlphaMode::Opaque => AlphaMode::Opaque,
            GltfAlphaMode::Mask => AlphaMode::Mask,
            GltfAlphaMode::Blend => AlphaMode::Blend,
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
        info.texture = texture_ref(id);
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

pub(crate) fn texture_ref(id: Uuid) -> TextureRef {
    if id.is_nil() || id == Uuid::max() { TextureRef::Clear } else { TextureRef::Asset(AssetId::new(id)) }
}

fn linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}
