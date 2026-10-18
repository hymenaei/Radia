//! Bounded surface appearance and per-face material descriptions.

use crate::AssetId;
use base::UUID;

/// The object's appearance category, independent from its protocol kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Appearance {
    Surface,
    Avatar,
}

/// Maximum per-face appearance index supported by the legacy TextureEntry format.
pub const MAX_SURFACE_FACES: usize = 45;
/// Maximum number of PBR face bindings supported by the 45 TextureEntry face slots.
pub const MAX_RENDER_MATERIAL_BINDINGS: usize = MAX_SURFACE_FACES;
/// Number of texture-info slots supported by a GLTF material override.
pub const GLTF_TEXTURE_INFO_COUNT: usize = 4;

/// Legacy per-face surface values decoded from a simulator TextureEntry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAppearance {
    /// Diffuse texture asset UUID.
    pub texture_id: UUID,
    /// Red, green, blue, and alpha bytes after applying the wire inversion.
    pub color: [u8; 4],
    /// Texture-coordinate scale along S and T.
    pub scale_s: f32,
    pub scale_t: f32,
    /// Texture-coordinate offsets in normalized viewer units.
    pub offset_s: f32,
    pub offset_t: f32,
    /// Rotation in radians.
    pub rotation: f32,
    /// Packed bump, shiny, and fullbright flags.
    pub bump_shiny_fullbright: u8,
    /// Packed media and texture-generation flags.
    pub media_texgen: u8,
    /// Glow in the range 0–1.
    pub glow: f32,
    /// Legacy material asset UUID, distinct from PBR render-material bindings.
    pub material_id: UUID,
}

impl Default for FaceAppearance {
    fn default() -> Self {
        Self {
            texture_id: UUID::nil(),
            color: [u8::MAX; 4],
            scale_s: 1.0,
            scale_t: 1.0,
            offset_s: 0.0,
            offset_t: 0.0,
            rotation: 0.0,
            bump_shiny_fullbright: 0,
            media_texgen: 0,
            glow: 0.0,
            material_id: UUID::nil(),
        }
    }
}

/// Bounded legacy appearance values for a surface object.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceAppearance {
    default_face: FaceAppearance,
    overrides: Vec<(u8, FaceAppearance)>,
}

impl SurfaceAppearance {
    /// Builds an appearance with sorted, unique overrides for face indices 0–44.
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

    /// Returns the value used when a face has no override.
    pub const fn default_face(&self) -> &FaceAppearance {
        &self.default_face
    }

    /// Returns a value for face indices 0 through 44.
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

    /// Returns sorted face overrides. Each index is unique and in the range 0–44.
    pub fn overrides(&self) -> impl Iterator<Item = (u8, &FaceAppearance)> {
        self.overrides.iter().map(|(index, face)| (*index, face))
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

/// Why a surface appearance could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceAppearanceError {
    /// A face index is outside the supported range 0–44.
    FaceIndexOutOfRange(u8),
    /// An override was supplied more than once for the same face.
    DuplicateFaceIndex(u8),
    /// More overrides were supplied than the 45 supported face slots.
    TooManyOverrides,
    /// A scale, offset, rotation, or glow value is not finite.
    NonFiniteValue,
}

/// Sparse PBR material asset bindings for TextureEntry faces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderMaterials {
    bindings: Vec<(u8, AssetId)>,
}

impl RenderMaterials {
    /// Builds sorted, unique bindings for up to 45 face indices in the range 0–44.
    ///
    /// Nil asset IDs and omitted indices represent faces without a PBR material binding.
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

    /// Returns the non-nil material for a supported face, if one is bound.
    ///
    /// Out-of-range indices and unbound faces return `None`.
    pub fn material(&self, face_index: u8) -> Option<AssetId> {
        if usize::from(face_index) >= MAX_SURFACE_FACES {
            return None;
        }
        self.bindings.binary_search_by_key(&face_index, |(index, _)| *index).ok().map(|slot| self.bindings[slot].1)
    }

    /// Returns sorted face bindings.
    pub fn bindings(&self) -> impl Iterator<Item = (u8, AssetId)> + '_ {
        self.bindings.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

/// Why RenderMaterials bindings could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderMaterialsError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyBindings,
}

/// GLTF alpha mode used by a material override.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfAlphaMode {
    Opaque,
    Blend,
    Mask,
}

/// Texture-coordinate transform values for one GLTF texture-info slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GltfTextureTransform {
    pub offset: Option<[f32; 2]>,
    pub scale: Option<[f32; 2]>,
    pub rotation: Option<f32>,
}

/// Verified GLTF material override fields for one TextureEntry face.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GltfMaterialOverride {
    /// Texture UUIDs for the four GLTF texture-info slots. `None` means omitted; a nil UUID is explicit.
    pub textures: [Option<UUID>; GLTF_TEXTURE_INFO_COUNT],
    /// Red, green, blue, and alpha base-color factors.
    pub base_color: Option<[f32; 4]>,
    /// Red, green, and blue emissive factors.
    pub emissive: Option<[f32; 3]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub alpha_mode: Option<GltfAlphaMode>,
    pub alpha_cutoff: Option<f32>,
    pub double_sided: Option<bool>,
    /// Optional transform values for each texture-info slot.
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

/// Complete typed GLTF material override snapshot keyed by TextureEntry face.
///
/// This snapshot may arrive before a face's base GLTF material is ready. World keeps overrides separate from
/// [`RenderMaterials`] so consumers can defer applying them until the base material is available.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GltfMaterialOverrides {
    faces: Vec<(u8, GltfMaterialOverride)>,
}

impl GltfMaterialOverrides {
    /// Builds a snapshot with sorted, unique overrides for face indices 0–44.
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

    /// Returns sorted face overrides.
    pub fn faces(&self) -> &[(u8, GltfMaterialOverride)] {
        &self.faces
    }

    pub fn face(&self, index: u8) -> Option<&GltfMaterialOverride> {
        self.faces.binary_search_by_key(&index, |(face, _)| *face).ok().map(|offset| &self.faces[offset].1)
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
}

/// Why a GLTF material override snapshot could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfMaterialOverridesError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyFaces,
    NonFiniteValue,
}
