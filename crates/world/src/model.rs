use std::num::{NonZeroU8, NonZeroU32};
use std::sync::atomic::{AtomicU64, Ordering};

use base::UUID;
use math::{DVec3, Quat, Vec3};

use crate::update::UpdateAuthority;

static NEXT_WORLD_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Entity(pub(super) hecs::Entity, pub(super) WorldId);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct WorldId(u64);

impl WorldId {
    pub(super) fn fresh() -> Self {
        let id = NEXT_WORLD_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
            .unwrap_or_else(|_| panic!("World identity space exhausted"));
        Self(id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Region(u64);

impl Region {
    /// Wraps the network connection identifier used for this region lifetime.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A simulator-local object address scoped to one connection lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LocalAddress {
    pub(super) region: Region,
    pub(super) local_id: NonZeroU32,
}

/// The accepted source that supplied one owner or creator value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PropertyProvenance {
    pub region: Region,
    pub authority: UpdateAuthority,
    pub order: u64,
    pub block_ordinal: u32,
    pub source_sequence: u32,
}

/// Provenance for independently refreshed owner and creator values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertiesProvenance {
    pub owner: Option<PropertyProvenance>,
    pub creator: Option<PropertyProvenance>,
}

/// Request-time identity used to reject property replies for a replaced object binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PropertiesTarget {
    pub(super) entity: Entity,
    pub(super) id: UUID,
    pub(super) address: LocalAddress,
    pub(super) address_generation: u64,
}

impl PropertiesTarget {
    pub const fn object_id(self) -> UUID {
        self.id
    }

    pub const fn address(self) -> LocalAddress {
        self.address
    }
}

impl LocalAddress {
    pub const fn new(region: Region, local_id: NonZeroU32) -> Self {
        Self { region, local_id }
    }

    pub const fn region(self) -> Region {
        self.region
    }

    pub const fn local_id(self) -> NonZeroU32 {
        self.local_id
    }
}

/// A packed Second Life region handle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegionHandle(u64);

impl RegionHandle {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn origin(self) -> DVec3 {
        DVec3::new((self.0 >> 32) as u32 as f64, self.0 as u32 as f64, 0.0)
    }
}

/// Optional, non-authoritative metadata for a registered region.
///
/// When passed to [`crate::World::patch_region`], `None` leaves the stored value unchanged. Supplied values cannot conflict with metadata already
/// stored for the region.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo {
    /// Second Life region UUID, when known.
    pub region_id: Option<UUID>,
    /// Global region handle, when known.
    pub handle: Option<RegionHandle>,
    /// Region name, when known.
    pub name: Option<String>,
    /// Region origin in global meters, when known.
    pub origin: Option<DVec3>,
}

/// A world-space position and orientation in meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldPose {
    pub position: DVec3,
    pub rotation: Quat,
}

/// A world-space pose with this object's accepted size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTransform {
    pub pose: WorldPose,
    pub scale: Vec3,
}

/// A resolved point on an avatar skeleton.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentPoint(NonZeroU8);

impl AttachmentPoint {
    pub const fn new(value: NonZeroU8) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

/// Optional sit-position evidence expressed relative to the owning object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SitTarget {
    pub position: Vec3,
    pub rotation: Quat,
}

/// The local frame used to interpret an object's accepted position and rotation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinateFrame {
    Unknown,
    PendingParent { address: LocalAddress, expected_uuid: Option<UUID> },
    RegionRelative { region: Region },
    LinksetRoot { root: Entity },
    AttachmentPoint { avatar: Entity, point: Option<AttachmentPoint> },
}

/// A pose query result, including unresolved and accepted-detach states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PoseState {
    Resolved(WorldPose),
    Unresolved(UnresolvedReason),
    Provisional { pose: WorldPose, reason: UnresolvedReason },
}

/// The first missing requirement in a pose's dependency chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnresolvedReason {
    PendingParent,
    PoseNotValidForCurrentFrame,
    MissingLocalPose,
    MissingAttachmentJoint,
    MissingRegionOrigin,
}

/// A defensive failure while resolving an internally inconsistent parent chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoseError {
    InvalidRotation { entity: Entity },
    ParentCycle,
    TraversalLimitExceeded { max_edges: usize },
}

/// A full transform query can be unavailable even when its pose is valid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldTransformState {
    Resolved(WorldTransform),
    Unresolved(PoseState),
    MissingScale(WorldPose),
}

/// A non-nil world identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Identity {
    pub(super) id: UUID,
}

impl Identity {
    pub const fn id(self) -> UUID {
        self.id
    }
}

/// An asset identity, distinct from the identity of an object using that asset.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetId(UUID);

impl AssetId {
    pub const fn new(uuid: UUID) -> Self {
        Self(uuid)
    }

    pub const fn uuid(self) -> UUID {
        self.0
    }
}

/// The protocol object category. Unknown PCode values remain available to callers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Kind {
    Avatar,
    Object,
    Unknown(u8),
}

/// Flags carried by sculpt and mesh volume parameters.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SculptFlags {
    pub invert: bool,
    pub mirror: bool,
}

/// A recognized sculpt geometry subtype.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SculptType {
    Sphere,
    Torus,
    Plane,
    Cylinder,
}

/// The object's shape and any verified sculpt or mesh asset data.
///
/// Sculpt and mesh variants carry asset data; matches should use struct patterns such as
/// `Shape::Sculpt { .. }` and `Shape::Mesh { .. }`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Shape {
    /// A regular volume without a sculpt or mesh asset.
    Prim,
    Sculpt {
        /// Sculpt texture asset; distinct from the object's identity.
        asset_id: AssetId,
        /// Recognized sculpt geometry subtype.
        sculpt_type: SculptType,
        /// Invert and mirror bits from the sculpt parameter.
        flags: SculptFlags,
    },
    Mesh {
        /// Mesh asset; distinct from the object's identity.
        asset_id: AssetId,
        /// Invert and mirror bits from the sculpt parameter.
        flags: SculptFlags,
    },
    Avatar,
}

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

/// Position, rotation, and scale can be learned independently.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    pub(super) position: Option<Vec3>,
    pub(super) rotation: Option<Quat>,
    pub(super) scale: Option<Vec3>,
}

impl Transform {
    pub const fn position(self) -> Option<Vec3> {
        self.position
    }

    pub const fn rotation(self) -> Option<Quat> {
        self.rotation
    }

    pub const fn scale(self) -> Option<Vec3> {
        self.scale
    }

    /// Returns a local transform only after all three source-relative values are known.
    pub const fn local_transform(self) -> Option<LocalTransform> {
        match (self.position, self.rotation, self.scale) {
            (Some(position), Some(rotation), Some(scale)) => Some(LocalTransform { position, rotation, scale }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalTransform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub position: Option<Vec3>,
    pub rotation: Option<Quat>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LinearMotion {
    pub velocity: Option<Vec3>,
    pub acceleration: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionPatch {
    pub velocity: Option<Vec3>,
    pub acceleration: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParentUpdate {
    Root,
    Local(NonZeroU32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Parent {
    Unknown,
    Root,
    Pending { address: LocalAddress, expected_uuid: Option<UUID> },
    Resolved(Entity),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Properties {
    pub owner_id: Option<UUID>,
    pub creator_id: Option<UUID>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertiesPatch {
    pub owner_id: Option<UUID>,
    pub creator_id: Option<UUID>,
}
