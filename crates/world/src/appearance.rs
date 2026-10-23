use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use utils::AssetId;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Appearance {
    Surface,
    Avatar,
}

pub const MAX_SURFACE_FACES: usize = 45;
pub const MAX_RENDER_MATERIAL_BINDINGS: usize = MAX_SURFACE_FACES;
pub const GLTF_TEXTURE_INFO_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAppearance {
    pub texture_id: Uuid,
    pub color: [u8; 4],
    pub scale_s: f32,
    pub scale_t: f32,
    pub offset_s: f32,
    pub offset_t: f32,
    pub rotation: f32,
    pub bump_shiny_fullbright: u8,
    pub media_texgen: u8,
    pub glow: f32,
    pub material_id: Uuid,
}

impl Default for FaceAppearance {
    fn default() -> Self {
        Self {
            texture_id: Uuid::nil(),
            color: [u8::MAX; 4],
            scale_s: 1.0,
            scale_t: 1.0,
            offset_s: 0.0,
            offset_t: 0.0,
            rotation: 0.0,
            bump_shiny_fullbright: 0,
            media_texgen: 0,
            glow: 0.0,
            material_id: Uuid::nil(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceAppearance {
    default_face: FaceAppearance,
    overrides: Vec<(u8, FaceAppearance)>,
}

impl SurfaceAppearance {
    pub fn new(default_face: FaceAppearance, mut overrides: Vec<(u8, FaceAppearance)>) -> Result<Self, SurfaceAppearanceError> {
        if !default_face.has_finite_values() || overrides.iter().any(|(_, face)| !face.has_finite_values()) {
            return Err(SurfaceAppearanceError::NonFiniteValue);
        }
        if overrides.len() > MAX_SURFACE_FACES {
            return Err(SurfaceAppearanceError::TooManyOverrides);
        }
        if let Some((index, _)) = overrides.iter().find(|(index, _)| usize::from(*index) >= MAX_SURFACE_FACES) {
            return Err(SurfaceAppearanceError::FaceIndexOutOfRange(*index));
        }

        overrides.sort_unstable_by_key(|(index, _)| *index);
        for pair in overrides.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(SurfaceAppearanceError::DuplicateFaceIndex(pair[0].0));
            }
        }
        overrides.retain(|(_, face)| *face != default_face);

        Ok(Self { default_face, overrides })
    }

    pub const fn default_face(&self) -> &FaceAppearance {
        &self.default_face
    }

    pub fn face(&self, index: u8) -> Option<&FaceAppearance> {
        if usize::from(index) >= MAX_SURFACE_FACES {
            return None;
        }
        Some(
            self.overrides
                .binary_search_by_key(&index, |(face_index, _)| *face_index)
                .map_or(&self.default_face, |slot| &self.overrides[slot].1),
        )
    }

    pub fn overrides(&self) -> impl Iterator<Item = (u8, &FaceAppearance)> {
        self.overrides.iter().map(|(index, face)| (*index, face))
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.overrides.capacity() * size_of::<(u8, FaceAppearance)>()
    }
}

impl FaceAppearance {
    fn has_finite_values(self) -> bool {
        self.scale_s.is_finite()
            && self.scale_t.is_finite()
            && self.offset_s.is_finite()
            && self.offset_t.is_finite()
            && self.rotation.is_finite()
            && self.glow.is_finite()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceAppearanceError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyOverrides,
    NonFiniteValue,
}

impl Display for SurfaceAppearanceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceIndexOutOfRange(index) => write!(f, "surface face index {index} is out of range"),
            Self::DuplicateFaceIndex(index) => write!(f, "surface face index {index} is duplicated"),
            Self::TooManyOverrides => f.write_str("too many surface face overrides"),
            Self::NonFiniteValue => f.write_str("surface appearance contains a non-finite value"),
        }
    }
}

impl Error for SurfaceAppearanceError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderMaterials {
    bindings: Vec<(u8, AssetId)>,
}

impl RenderMaterials {
    pub fn new(mut bindings: Vec<(u8, AssetId)>) -> Result<Self, RenderMaterialsError> {
        if bindings.len() > MAX_RENDER_MATERIAL_BINDINGS {
            return Err(RenderMaterialsError::TooManyBindings);
        }
        if let Some((index, _)) = bindings.iter().find(|(index, _)| usize::from(*index) >= MAX_SURFACE_FACES) {
            return Err(RenderMaterialsError::FaceIndexOutOfRange(*index));
        }

        bindings.sort_unstable_by_key(|(index, _)| *index);
        for pair in bindings.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(RenderMaterialsError::DuplicateFaceIndex(pair[0].0));
            }
        }
        bindings.retain(|(_, asset_id)| !asset_id.uuid().is_nil());

        Ok(Self { bindings })
    }

    pub fn material(&self, face_index: u8) -> Option<AssetId> {
        if usize::from(face_index) >= MAX_SURFACE_FACES {
            return None;
        }
        self.bindings.binary_search_by_key(&face_index, |(index, _)| *index).ok().map(|slot| self.bindings[slot].1)
    }

    pub fn bindings(&self) -> impl Iterator<Item = (u8, AssetId)> + '_ {
        self.bindings.iter().copied()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.bindings.capacity() * size_of::<(u8, AssetId)>()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderMaterialsError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyBindings,
}

impl Display for RenderMaterialsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceIndexOutOfRange(index) => write!(f, "material face index {index} is out of range"),
            Self::DuplicateFaceIndex(index) => write!(f, "material face index {index} is duplicated"),
            Self::TooManyBindings => f.write_str("too many material bindings"),
        }
    }
}

impl Error for RenderMaterialsError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfAlphaMode {
    Opaque,
    Blend,
    Mask,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GltfTextureTransform {
    pub offset: Option<[f32; 2]>,
    pub scale: Option<[f32; 2]>,
    pub rotation: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GltfMaterialOverride {
    pub textures: [Option<Uuid>; GLTF_TEXTURE_INFO_COUNT],
    pub base_color: Option<[f32; 4]>,
    pub emissive: Option<[f32; 3]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub alpha_mode: Option<GltfAlphaMode>,
    pub alpha_cutoff: Option<f32>,
    pub double_sided: Option<bool>,
    pub texture_transforms: [GltfTextureTransform; GLTF_TEXTURE_INFO_COUNT],
}

impl GltfMaterialOverride {
    fn has_finite_values(self) -> bool {
        self.base_color.is_none_or(|value| value.into_iter().all(f32::is_finite))
            && self.emissive.is_none_or(|value| value.into_iter().all(f32::is_finite))
            && self.metallic.is_none_or(f32::is_finite)
            && self.roughness.is_none_or(f32::is_finite)
            && self.alpha_cutoff.is_none_or(f32::is_finite)
            && self.texture_transforms.iter().all(|transform| {
                transform.offset.is_none_or(|value| value.into_iter().all(f32::is_finite))
                    && transform.scale.is_none_or(|value| value.into_iter().all(f32::is_finite))
                    && transform.rotation.is_none_or(f32::is_finite)
            })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GltfMaterialOverrides {
    faces: Vec<(u8, GltfMaterialOverride)>,
}

impl GltfMaterialOverrides {
    pub fn new(mut faces: Vec<(u8, GltfMaterialOverride)>) -> Result<Self, GltfMaterialOverridesError> {
        if faces.len() > MAX_SURFACE_FACES {
            return Err(GltfMaterialOverridesError::TooManyFaces);
        }
        if let Some((index, _)) = faces.iter().find(|(index, _)| usize::from(*index) >= MAX_SURFACE_FACES) {
            return Err(GltfMaterialOverridesError::FaceIndexOutOfRange(*index));
        }
        faces.sort_unstable_by_key(|(index, _)| *index);
        if let Some(pair) = faces.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(GltfMaterialOverridesError::DuplicateFaceIndex(pair[0].0));
        }
        if faces.iter().any(|(_, material)| !material.has_finite_values()) {
            return Err(GltfMaterialOverridesError::NonFiniteValue);
        }
        Ok(Self { faces })
    }

    pub fn faces(&self) -> &[(u8, GltfMaterialOverride)] {
        &self.faces
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.faces.capacity() * size_of::<(u8, GltfMaterialOverride)>()
    }

    pub fn face(&self, index: u8) -> Option<&GltfMaterialOverride> {
        self.faces.binary_search_by_key(&index, |(face, _)| *face).ok().map(|offset| &self.faces[offset].1)
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfMaterialOverridesError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyFaces,
    NonFiniteValue,
}

impl Display for GltfMaterialOverridesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceIndexOutOfRange(index) => write!(f, "GLTF material face index {index} is out of range"),
            Self::DuplicateFaceIndex(index) => write!(f, "GLTF material face index {index} is duplicated"),
            Self::TooManyFaces => f.write_str("too many GLTF material faces"),
            Self::NonFiniteValue => f.write_str("GLTF material override contains a non-finite value"),
        }
    }
}

impl Error for GltfMaterialOverridesError {}
