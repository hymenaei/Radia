use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use math::UvTransform;
use utils::AssetId;
use uuid::Uuid;

use crate::FieldUpdate;

pub const MAX_SURFACE_FACES: usize = 45;
pub const MAX_PBR_MATERIAL_BINDINGS: usize = MAX_SURFACE_FACES;

const TEXTURE_ANIMATION_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Appearance {
    Surface,
    Avatar,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureAnimation {
    mode: u8,
    face: i8,
    size_x: u8,
    size_y: u8,
    start: f32,
    length: f32,
    rate: f32,
}

impl TextureAnimation {
    pub const ON: u8 = 0x01;
    pub const LOOP: u8 = 0x02;
    pub const REVERSE: u8 = 0x04;
    pub const PING_PONG: u8 = 0x08;
    pub const SMOOTH: u8 = 0x10;
    pub const ROTATE: u8 = 0x20;
    pub const SCALE: u8 = 0x40;

    pub fn from_wire(bytes: &[u8]) -> Result<Self, TextureAnimationError> {
        if bytes.len() != TEXTURE_ANIMATION_BYTES {
            return Err(TextureAnimationError::InvalidLength);
        }
        let mode = *bytes.first().ok_or(TextureAnimationError::InvalidLength)?;
        let face = i8::from_ne_bytes([*bytes.get(1).ok_or(TextureAnimationError::InvalidLength)?]);
        if face < -1 || face >= MAX_SURFACE_FACES as i8 {
            return Err(TextureAnimationError::InvalidFace);
        }
        let raw_size_x = *bytes.get(2).ok_or(TextureAnimationError::InvalidLength)?;
        let raw_size_y = *bytes.get(3).ok_or(TextureAnimationError::InvalidLength)?;
        let (size_x, size_y) = if mode & Self::SMOOTH == 0 { (raw_size_x.max(1), raw_size_y.max(1)) } else { (raw_size_x, raw_size_y) };
        let value =
            Self { mode, face, size_x, size_y, start: wire_f32(bytes, 4)?, length: wire_f32(bytes, 8)?, rate: wire_f32(bytes, 12)? };
        if ![value.start, value.length, value.rate].into_iter().all(f32::is_finite) {
            return Err(TextureAnimationError::NonFiniteValue);
        }
        if value.length < 0.0 {
            return Err(TextureAnimationError::InvalidFrameCount);
        }
        Ok(value)
    }

    pub const fn mode(self) -> u8 {
        self.mode
    }

    pub const fn face(self) -> i8 {
        self.face
    }

    pub const fn size(self) -> [u8; 2] {
        [self.size_x, self.size_y]
    }

    pub const fn start(self) -> f32 {
        self.start
    }

    pub const fn length(self) -> f32 {
        self.length
    }

    pub const fn rate(self) -> f32 {
        self.rate
    }

    pub const fn is_enabled(self) -> bool {
        self.mode & Self::ON != 0
    }

    pub const fn is_smooth(self) -> bool {
        self.mode & Self::SMOOTH != 0
    }

    pub fn targets_face(self, face: u8) -> bool {
        self.face == -1 || u8::try_from(self.face).is_ok_and(|target| target == face)
    }

    fn num_frames(self) -> f32 {
        if self.length != 0.0 { self.length } else { (f32::from(self.size_x) * f32::from(self.size_y)).max(1.0) }
    }

    fn full_length(self) -> f32 {
        let num_frames = self.num_frames();
        if self.mode & Self::PING_PONG != 0 {
            if self.is_smooth() {
                2.0 * num_frames
            } else if self.mode & Self::LOOP != 0 {
                (2.0 * num_frames - 2.0).max(1.0)
            } else {
                (2.0 * num_frames - 1.0).max(1.0)
            }
        } else {
            num_frames
        }
    }

    pub fn transform_at(self, elapsed_seconds: f64, smooth_frame_offset: f64, base: UvTransform) -> UvTransform {
        if !self.is_enabled() || !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 || !smooth_frame_offset.is_finite() {
            return base;
        }

        let num_frames = self.num_frames();
        let full_length = self.full_length();
        if !full_length.is_finite() || full_length <= 0.0 {
            return base;
        }

        let elapsed = elapsed_seconds as f32;
        let frame_offset = if self.is_smooth() { smooth_frame_offset as f32 } else { 0.0 };
        let mut frame = elapsed * self.rate + frame_offset;
        if !frame.is_finite() {
            return base;
        }
        if self.mode & Self::LOOP != 0 {
            frame %= full_length;
        } else {
            frame = frame.min(full_length - 1.0);
        }
        if !self.is_smooth() {
            frame = (frame + 0.01).floor().min(full_length - 1.0);
        }
        if self.mode & Self::PING_PONG != 0 && frame >= num_frames {
            frame = if self.is_smooth() { num_frames - (frame - num_frames) } else { (num_frames - 1.99) - (frame - num_frames) };
        }
        if self.mode & Self::REVERSE != 0 {
            frame = if self.is_smooth() { num_frames - frame } else { (num_frames - 0.99) - frame };
        }
        frame += self.start;
        if !self.is_smooth() {
            frame = (frame + 0.5).floor();
        }
        if !frame.is_finite() {
            return base;
        }

        let mut result = base;
        if self.mode & Self::ROTATE != 0 {
            result.rotation = frame;
        } else if self.mode & Self::SCALE != 0 {
            result.scale = [frame; 2];
        } else if self.size_x != 0 && self.size_y != 0 {
            let size_x = f32::from(self.size_x);
            let size_y = f32::from(self.size_y);
            let scale = [1.0 / size_x, 1.0 / size_y];
            let x_frame = frame % size_x;
            let y_frame = (frame / size_x).trunc().clamp(i32::MIN as f32, i32::MAX as f32);
            result.scale = scale;
            result.offset = [(-0.5 + 0.5 * scale[0]) + x_frame * scale[0], (0.5 - 0.5 * scale[1]) - y_frame * scale[1]];
        } else {
            result.scale = [1.0; 2];
            result.offset = [-0.5 + 0.5 + frame, 0.0];
        }
        if result.is_finite() { result } else { base }
    }

    pub fn smooth_frame_counter(self, elapsed_seconds: f64, smooth_frame_offset: f64) -> Option<f64> {
        if !self.is_smooth() || !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 || !smooth_frame_offset.is_finite() {
            return None;
        }
        let frame = elapsed_seconds as f32 * self.rate + smooth_frame_offset as f32;
        frame.is_finite().then_some(f64::from(frame))
    }

    pub fn is_changing_at(self, elapsed_seconds: f64, smooth_frame_offset: f64) -> bool {
        if !self.is_enabled() || self.rate == 0.0 || !elapsed_seconds.is_finite() || elapsed_seconds < 0.0 {
            return false;
        }
        let full_length = self.full_length();
        if !full_length.is_finite() || full_length <= 0.0 {
            return false;
        }
        if self.mode & Self::LOOP != 0 {
            return self.is_smooth() || full_length > 1.0;
        }
        if self.rate < 0.0 {
            return true;
        }
        let offset = if self.is_smooth() { smooth_frame_offset as f32 } else { 0.0 };
        let frame = elapsed_seconds as f32 * self.rate + offset;
        frame.is_finite() && frame < full_length - 1.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureAnimationError {
    InvalidLength,
    InvalidFace,
    NonFiniteValue,
    InvalidFrameCount,
}

impl Display for TextureAnimationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLength => "texture animation block must contain 16 bytes",
            Self::InvalidFace => "texture animation face must be -1 or a supported face index",
            Self::NonFiniteValue => "texture animation contains a non-finite value",
            Self::InvalidFrameCount => "texture animation length must be non-negative",
        })
    }
}

impl Error for TextureAnimationError {}

fn wire_f32(bytes: &[u8], offset: usize) -> Result<f32, TextureAnimationError> {
    let end = offset.checked_add(4).ok_or(TextureAnimationError::InvalidLength)?;
    let raw: [u8; 4] =
        bytes.get(offset..end).ok_or(TextureAnimationError::InvalidLength)?.try_into().map_err(|_| TextureAnimationError::InvalidLength)?;
    Ok(f32::from_le_bytes(raw))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextureMapping {
    #[default]
    Default,
    Planar,
    Unsupported(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceAppearance {
    pub texture: Option<AssetId>,
    pub color: [u8; 4],
    pub transform: UvTransform,
    pub bump: u8,
    pub shiny: u8,
    pub fullbright: bool,
    pub mapping: TextureMapping,
    pub media: bool,
    pub glow: f32,
    pub material: Option<AssetId>,
}

impl Default for FaceAppearance {
    fn default() -> Self {
        Self {
            texture: None,
            color: [u8::MAX; 4],
            transform: UvTransform::default(),
            bump: 0,
            shiny: 0,
            fullbright: false,
            mapping: TextureMapping::Default,
            media: false,
            glow: 0.0,
            material: None,
        }
    }
}

impl FaceAppearance {
    fn validate(self) -> Result<(), SurfaceAppearanceError> {
        if !self.transform.is_finite() || !self.glow.is_finite() {
            return Err(SurfaceAppearanceError::NonFiniteValue);
        }
        if self.texture.is_some_and(|id| id.uuid().is_nil() || id.uuid() == Uuid::max()) {
            return Err(SurfaceAppearanceError::InvalidValue("texture"));
        }
        if self.material.is_some_and(|id| id.uuid().is_nil()) {
            return Err(SurfaceAppearanceError::InvalidValue("material"));
        }
        if self.bump > 31 {
            return Err(SurfaceAppearanceError::InvalidValue("bump"));
        }
        if self.shiny > 3 {
            return Err(SurfaceAppearanceError::InvalidValue("shiny"));
        }
        if matches!(self.mapping, TextureMapping::Unsupported(0 | 1)) {
            return Err(SurfaceAppearanceError::InvalidValue("mapping"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceAppearance {
    default_face: FaceAppearance,
    overrides: Vec<(u8, FaceAppearance)>,
}

impl SurfaceAppearance {
    pub fn new(default_face: FaceAppearance, mut overrides: Vec<(u8, FaceAppearance)>) -> Result<Self, SurfaceAppearanceError> {
        if overrides.len() > MAX_SURFACE_FACES {
            return Err(SurfaceAppearanceError::TooManyOverrides);
        }
        default_face.validate()?;
        for (_, face) in &overrides {
            face.validate()?;
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
        self.overrides.len() * size_of::<(u8, FaceAppearance)>()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceAppearanceError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyOverrides,
    NonFiniteValue,
    InvalidValue(&'static str),
}

impl Display for SurfaceAppearanceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceIndexOutOfRange(index) => write!(f, "surface face index {index} is out of range"),
            Self::DuplicateFaceIndex(index) => write!(f, "surface face index {index} is duplicated"),
            Self::TooManyOverrides => f.write_str("too many surface face overrides"),
            Self::NonFiniteValue => f.write_str("surface appearance contains a non-finite value"),
            Self::InvalidValue(field) => write!(f, "surface appearance contains an invalid {field}"),
        }
    }
}

impl Error for SurfaceAppearanceError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PbrMaterialAssignments {
    bindings: Vec<(u8, AssetId)>,
}

impl PbrMaterialAssignments {
    pub fn new(mut bindings: Vec<(u8, AssetId)>) -> Result<Self, PbrMaterialAssignmentsError> {
        if bindings.len() > MAX_PBR_MATERIAL_BINDINGS {
            return Err(PbrMaterialAssignmentsError::TooManyBindings);
        }
        if let Some((index, _)) = bindings.iter().find(|(index, _)| usize::from(*index) >= MAX_SURFACE_FACES) {
            return Err(PbrMaterialAssignmentsError::FaceIndexOutOfRange(*index));
        }

        bindings.sort_unstable_by_key(|(index, _)| *index);
        for pair in bindings.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(PbrMaterialAssignmentsError::DuplicateFaceIndex(pair[0].0));
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
        self.bindings.len() * size_of::<(u8, AssetId)>()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PbrMaterialAssignmentsError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyBindings,
}

impl Display for PbrMaterialAssignmentsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceIndexOutOfRange(index) => write!(f, "material face index {index} is out of range"),
            Self::DuplicateFaceIndex(index) => write!(f, "material face index {index} is duplicated"),
            Self::TooManyBindings => f.write_str("too many material bindings"),
        }
    }
}

impl Error for PbrMaterialAssignmentsError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PbrAlphaMode {
    Opaque,
    Blend,
    Mask,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PbrTextureTransformOverride {
    pub offset: Option<[f32; 2]>,
    pub scale: Option<[f32; 2]>,
    pub rotation: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PbrTextureOverride {
    pub texture: FieldUpdate<AssetId>,
    pub transform: PbrTextureTransformOverride,
}

impl Default for PbrTextureOverride {
    fn default() -> Self {
        Self { texture: FieldUpdate::Keep, transform: PbrTextureTransformOverride::default() }
    }
}

impl PbrTextureOverride {
    fn validate(self) -> Result<(), PbrMaterialOverridesError> {
        if matches!(self.texture, FieldUpdate::Set(id) if id.uuid().is_nil() || id.uuid() == Uuid::max()) {
            return Err(PbrMaterialOverridesError::InvalidTextureId);
        }
        if !self.transform.offset.is_none_or(|value| value.into_iter().all(f32::is_finite))
            || !self.transform.scale.is_none_or(|value| value.into_iter().all(f32::is_finite))
            || !self.transform.rotation.is_none_or(f32::is_finite)
        {
            return Err(PbrMaterialOverridesError::NonFiniteValue);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PbrMaterialOverride {
    pub base_color: PbrTextureOverride,
    pub normal: PbrTextureOverride,
    pub metallic_roughness: PbrTextureOverride,
    pub emissive: PbrTextureOverride,
    pub base_color_factor: Option<[f32; 4]>,
    pub emissive_factor: Option<[f32; 3]>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub alpha_mode: Option<PbrAlphaMode>,
    pub alpha_cutoff: Option<f32>,
    pub double_sided: Option<bool>,
}

impl PbrMaterialOverride {
    fn validate(self) -> Result<(), PbrMaterialOverridesError> {
        if !self.base_color_factor.is_none_or(|value| value.into_iter().all(f32::is_finite))
            || !self.emissive_factor.is_none_or(|value| value.into_iter().all(f32::is_finite))
            || !self.metallic.is_none_or(f32::is_finite)
            || !self.roughness.is_none_or(f32::is_finite)
            || !self.alpha_cutoff.is_none_or(f32::is_finite)
        {
            return Err(PbrMaterialOverridesError::NonFiniteValue);
        }
        for texture in [self.base_color, self.normal, self.metallic_roughness, self.emissive] {
            texture.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PbrMaterialOverrides {
    faces: Vec<(u8, PbrMaterialOverride)>,
}

impl PbrMaterialOverrides {
    pub fn new(mut faces: Vec<(u8, PbrMaterialOverride)>) -> Result<Self, PbrMaterialOverridesError> {
        if faces.len() > MAX_SURFACE_FACES {
            return Err(PbrMaterialOverridesError::TooManyFaces);
        }
        if let Some((index, _)) = faces.iter().find(|(index, _)| usize::from(*index) >= MAX_SURFACE_FACES) {
            return Err(PbrMaterialOverridesError::FaceIndexOutOfRange(*index));
        }
        faces.sort_unstable_by_key(|(index, _)| *index);
        if let Some(pair) = faces.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(PbrMaterialOverridesError::DuplicateFaceIndex(pair[0].0));
        }
        for (_, material) in &faces {
            material.validate()?;
        }
        Ok(Self { faces })
    }

    pub fn faces(&self) -> &[(u8, PbrMaterialOverride)] {
        &self.faces
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.faces.len() * size_of::<(u8, PbrMaterialOverride)>()
    }

    pub fn face(&self, index: u8) -> Option<&PbrMaterialOverride> {
        self.faces.binary_search_by_key(&index, |(face, _)| *face).ok().map(|offset| &self.faces[offset].1)
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PbrMaterialOverridesError {
    FaceIndexOutOfRange(u8),
    DuplicateFaceIndex(u8),
    TooManyFaces,
    NonFiniteValue,
    InvalidTextureId,
}

impl Display for PbrMaterialOverridesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::FaceIndexOutOfRange(index) => write!(f, "PBR material face index {index} is out of range"),
            Self::DuplicateFaceIndex(index) => write!(f, "PBR material face index {index} is duplicated"),
            Self::TooManyFaces => f.write_str("too many PBR material faces"),
            Self::NonFiniteValue => f.write_str("PBR material override contains a non-finite value"),
            Self::InvalidTextureId => f.write_str("PBR texture override contains an invalid asset ID"),
        }
    }
}

impl Error for PbrMaterialOverridesError {}
