#![allow(dead_code, unused_imports)]

pub(super) use std::{collections::HashSet, num::NonZeroU32};

pub(super) use math::{Quat, Vec3};
pub(super) use utils::AssetId;
pub(super) use uuid::Uuid;
pub(super) use world::ApplyError::{InvalidValue, KindMismatch};
pub(super) use world::ApplyOutcome::{AgentHandedOff, Deferred, IgnoredStale, Introduced, Removed, TombstoneRecorded, Unchanged, Updated};
pub(super) use world::DeferredReason::{AmbiguousOrder, UnknownObject};
pub(super) use world::FieldGroup;
pub(super) use world::FieldGroup::{AngularVelocity, Scale};
pub(super) use world::FieldRejectionReason::{InvalidRotation, ParentCycle, ParentDependsOnRejectedParent, UntrustedSource};
pub(super) use world::Kind::{Avatar, Object, Unknown};
pub(super) use world::OrderEvidence::{Ambiguous, Degraded, Ordered};
pub(super) use world::Parent;
pub(super) use world::Parent::{Pending, Resolved};
pub(super) use world::ParentUpdate::{Local, Root};
pub(super) use world::{Appearance, Shape};
pub(super) use world::{
    AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, FaceAppearance, FieldRejection,
    FieldRejectionReason, FieldUpdate, Kind, LinearMotion, LocalAddress, MAX_AVATAR_TEXTURE_SLOTS, MAX_PBR_MATERIAL_BINDINGS, MediaEntry,
    ObjectMediaUpdate, ObjectObservation, ObjectPatch, ParentUpdate, PbrMaterialAssignments, PbrMaterialAssignmentsError, Pose,
    PropertiesPatch, Region, RegionInfo, SculptFlags, SculptType, SitTarget, SurfaceAppearance, SurfaceAppearanceError, UpdateAuthority,
    UpdateSource, World, WorldLimits,
};

pub(super) fn limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 4,
        max_entities: 8,
        max_entities_per_region: 8,
        max_tombstones_per_region: 4,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: 4,
    }
}

pub(super) fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

pub(super) fn patch_properties_current(
    world: &mut World,
    source: UpdateSource,
    id: Uuid,
    patch: PropertiesPatch,
) -> Result<world::Applied, world::ApplyError> {
    let entity = world.entity(id).expect("property patch target is live");
    let target = world.object_binding_target(entity).expect("live property target has a current binding");
    world.patch_properties(source, target, id, patch)
}

pub(super) fn surface(default_texture: u128, face_texture: u128) -> SurfaceAppearance {
    SurfaceAppearance::new(
        FaceAppearance { texture: Some(AssetId::new(uuid(default_texture))), ..FaceAppearance::default() },
        vec![(44, FaceAppearance { texture: Some(AssetId::new(uuid(face_texture))), ..FaceAppearance::default() })],
    )
    .expect("valid face overrides")
}

pub(super) fn pbr_materials(default_material: u128, face_material: u128) -> PbrMaterialAssignments {
    PbrMaterialAssignments::new(vec![(0, AssetId::new(uuid(default_material))), (44, AssetId::new(uuid(face_material)))])
        .expect("valid PBR face bindings")
}

pub(super) fn sculpt_shape(asset: u128) -> Shape {
    Shape::Sculpt { asset_id: AssetId::new(uuid(asset)), sculpt_type: SculptType::Sphere, flags: SculptFlags::default() }
}

pub(super) fn mesh_shape(asset: u128) -> Shape {
    Shape::Mesh { asset_id: AssetId::new(uuid(asset)), flags: SculptFlags::default() }
}

pub(super) fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID must be nonzero")
}

pub(super) fn address(region: Region, local_id_value: u32) -> LocalAddress {
    LocalAddress::new(region, local_id(local_id_value))
}

pub(super) fn source(region: Region, order: u64) -> UpdateSource {
    ordered(region, order, 0)
}

pub(super) fn ordered(region: Region, order: u64, block_ordinal: u32) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), block_ordinal)
}

pub(super) fn untracked(region: Region, order: u64) -> UpdateSource {
    source(region, order).with_authority(UpdateAuthority::Untracked)
}

pub(super) fn object(source: UpdateSource, id: u128, local_id_value: u32, kind: Kind, parent: ParentUpdate) -> ObjectObservation {
    ObjectObservation::new(source, uuid(id), local_id(local_id_value), kind, parent)
}

pub(super) fn with_region(region: Region) -> World {
    let mut world = World::new(limits());
    assert!(matches!(world.adopt_region(region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    world
}
