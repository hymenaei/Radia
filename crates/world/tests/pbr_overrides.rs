use std::num::NonZeroU32;

use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance,
    ApplyOutcome::{Introduced, Unchanged, Updated},
    FieldGroup, FieldRejectionReason, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate,
    PbrMaterialAssignments, PbrMaterialOverride, PbrMaterialOverrides, Region, UpdateAuthority, UpdateSource, World, WorldLimits,
};

fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, order as u32, OrderEvidence::Ordered(order), 0)
}

fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID is nonzero")
}

fn with_region(region: Region) -> World {
    let mut world = World::new(WorldLimits {
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
    });
    assert!(matches!(world.adopt_region(region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    world
}

fn snapshot(face: u8, roughness: f32) -> PbrMaterialOverrides {
    PbrMaterialOverrides::new(vec![(face, PbrMaterialOverride { roughness: Some(roughness), ..PbrMaterialOverride::default() })])
        .expect("valid override snapshot")
}

fn object(source: UpdateSource, id: u128, local_id_value: u32, with_surface: bool) -> ObjectObservation {
    ObjectObservation {
        appearance: if with_surface { FieldUpdate::Set(Appearance::Surface) } else { FieldUpdate::Keep },
        pbr_materials: Some(PbrMaterialAssignments::new(vec![(2, AssetId::new(Uuid::from_u128(29)))]).expect("valid PBR bindings")),
        ..ObjectObservation::new(source, Uuid::from_u128(id), local_id(local_id_value), Kind::Object, ParentUpdate::Root)
    }
}

#[test]
fn snapshot_replaces_and_clears_only_overrides() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, true)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let mut first = ObjectPatch::new(source(region, 2), local_id(7));
    first.pbr_material_overrides = Some(snapshot(1, 0.2));
    assert_eq!(world.patch_object(first).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.2));

    let mut replacement = ObjectPatch::new(source(region, 3), local_id(7));
    replacement.pbr_material_overrides = Some(snapshot(4, 0.8));
    assert_eq!(world.patch_object(replacement).unwrap().outcome, Updated(entity));
    {
        let object = world.object(entity).unwrap();
        let overrides = object.pbr_material_overrides().unwrap();
        assert_eq!(overrides.face(1), None);
        assert_eq!(overrides.face(4).unwrap().roughness, Some(0.8));
        assert_eq!(object.pbr_materials().unwrap().material(2).unwrap().uuid(), Uuid::from_u128(29));
    }

    let mut stale = ObjectPatch::new(source(region, 2), local_id(7));
    stale.pbr_material_overrides = Some(snapshot(8, 0.1));
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides().unwrap().face(4).unwrap().roughness, Some(0.8));

    let mut clear = ObjectPatch::new(source(region, 4), local_id(7));
    clear.pbr_material_overrides = Some(PbrMaterialOverrides::default());
    assert_eq!(world.patch_object(clear).unwrap().outcome, Updated(entity));
    let object = world.object(entity).unwrap();
    assert_eq!(object.pbr_material_overrides(), None);
    assert_eq!(object.pbr_materials().unwrap().material(2).unwrap().uuid(), Uuid::from_u128(29));
}

#[test]
fn override_is_retained_before_base_binding() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, true)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let mut override_update = ObjectPatch::new(source(region, 2), local_id(7));
    override_update.pbr_material_overrides = Some(snapshot(1, 0.4));
    assert_eq!(world.patch_object(override_update).unwrap().outcome, Updated(entity));

    {
        let current = world.object(entity).unwrap();
        assert_eq!(current.pbr_materials().unwrap().material(1), None);
        assert_eq!(current.pbr_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.4));
    }

    let mut material_update = ObjectPatch::new(source(region, 3), local_id(7));
    material_update.pbr_materials = Some(PbrMaterialAssignments::new(vec![(1, AssetId::new(Uuid::from_u128(31)))]).expect("valid binding"));
    assert_eq!(world.patch_object(material_update).unwrap().outcome, Updated(entity));

    let current = world.object(entity).unwrap();
    assert_eq!(current.pbr_materials().unwrap().material(1).unwrap().uuid(), Uuid::from_u128(31));
    assert_eq!(current.pbr_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.4));
}

#[test]
fn rejects_overrides_without_surface_appearance() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, false)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    let mut patch = ObjectPatch::new(source(region, 2), local_id(7));
    patch.pbr_material_overrides = Some(snapshot(1, 0.2));

    let applied = world.patch_object(patch).unwrap();

    assert!(applied.rejections.iter().any(|rejection| {
        rejection.group == FieldGroup::PbrMaterialOverrides
            && rejection.reason == FieldRejectionReason::InvalidValue { field: "pbr_material_overrides" }
    }));
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides(), None);
}

#[test]
fn untracked_snapshot_cannot_replace_tracked_data() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, true)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    let mut tracked = ObjectPatch::new(source(region, 2), local_id(7));
    tracked.pbr_material_overrides = Some(snapshot(1, 0.4));
    assert_eq!(world.patch_object(tracked).unwrap().outcome, Updated(entity));

    let untracked_source = source(region, 3).with_authority(UpdateAuthority::Untracked);
    let mut untracked = ObjectPatch::new(untracked_source, local_id(7));
    untracked.pbr_material_overrides = Some(snapshot(1, 0.9));
    let applied = world.patch_object(untracked).unwrap();

    assert!(applied.rejections.iter().any(|rejection| {
        rejection.group == FieldGroup::PbrMaterialOverrides && rejection.reason == FieldRejectionReason::UntrustedSource
    }));
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.4));
}

#[test]
fn local_id_is_scoped_to_region() {
    let first_region = Region::new(11);
    let second_region = Region::new(12);
    let mut world = with_region(first_region);
    assert!(matches!(world.adopt_region(second_region).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    let first = world.observe_object(object(source(first_region, 1), 17, 7, true)).unwrap();
    let Introduced(first_entity) = first.outcome else { panic!("expected first object") };
    let second = world.observe_object(object(source(second_region, 1), 18, 7, true)).unwrap();
    let Introduced(second_entity) = second.outcome else { panic!("expected second object") };
    let mut patch = ObjectPatch::new(source(second_region, 2), local_id(7));
    patch.pbr_material_overrides = Some(snapshot(3, 0.7));

    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(second_entity));

    assert_eq!(world.object(first_entity).unwrap().pbr_material_overrides(), None);
    assert_eq!(world.object(second_entity).unwrap().pbr_material_overrides().unwrap().face(3).unwrap().roughness, Some(0.7));
}

#[test]
fn texture_overrides_validate_each_slot() {
    use world::{PbrMaterialOverridesError, PbrTextureOverride, PbrTextureTransformOverride};

    for (texture, error) in [
        (
            PbrTextureOverride { texture: FieldUpdate::Set(AssetId::new(Uuid::nil())), ..Default::default() },
            PbrMaterialOverridesError::InvalidTextureId,
        ),
        (
            PbrTextureOverride { texture: FieldUpdate::Set(AssetId::new(Uuid::max())), ..Default::default() },
            PbrMaterialOverridesError::InvalidTextureId,
        ),
        (
            PbrTextureOverride {
                transform: PbrTextureTransformOverride { offset: Some([f32::NAN, 0.0]), ..Default::default() },
                ..Default::default()
            },
            PbrMaterialOverridesError::NonFiniteValue,
        ),
        (
            PbrTextureOverride {
                transform: PbrTextureTransformOverride { scale: Some([1.0, f32::INFINITY]), ..Default::default() },
                ..Default::default()
            },
            PbrMaterialOverridesError::NonFiniteValue,
        ),
        (
            PbrTextureOverride {
                transform: PbrTextureTransformOverride { rotation: Some(f32::NAN), ..Default::default() },
                ..Default::default()
            },
            PbrMaterialOverridesError::NonFiniteValue,
        ),
    ] {
        for material in [
            PbrMaterialOverride { base_color: texture, ..Default::default() },
            PbrMaterialOverride { normal: texture, ..Default::default() },
            PbrMaterialOverride { metallic_roughness: texture, ..Default::default() },
            PbrMaterialOverride { emissive: texture, ..Default::default() },
        ] {
            assert_eq!(PbrMaterialOverrides::new(vec![(0, material)]), Err(error));
        }
    }
    for texture in [FieldUpdate::Keep, FieldUpdate::Clear, FieldUpdate::Set(AssetId::new(Uuid::from_u128(7)))] {
        let material = PbrMaterialOverride { base_color: PbrTextureOverride { texture, ..Default::default() }, ..Default::default() };
        let overrides = PbrMaterialOverrides::new(vec![(0, material)]).unwrap();
        assert_eq!(overrides.face(0), Some(&material));
    }
}

#[test]
fn surface_patches_use_accepted_classification() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, false)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    assert_eq!(world.object(entity).unwrap().pbr_materials().unwrap().material(2), Some(AssetId::new(Uuid::from_u128(29))));
    assert!(introduced.rejections.is_empty());

    let materials = PbrMaterialAssignments::new(vec![(0, AssetId::new(Uuid::from_u128(31)))]).unwrap();
    let mut patch = ObjectPatch::new(source(region, 2), local_id(7));
    patch.appearance = FieldUpdate::Set(Appearance::Surface);
    patch.pbr_materials = Some(materials.clone());
    patch.pbr_material_overrides = Some(snapshot(0, 0.4));
    let applied = world.patch_object(patch).unwrap();
    assert_eq!(applied.outcome, Updated(entity));
    assert!(applied.rejections.is_empty());
    assert_eq!(world.object(entity).unwrap().pbr_materials(), Some(&materials));
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides().unwrap().face(0).unwrap().roughness, Some(0.4));

    let mut patch = ObjectPatch::new(source(region, 10), local_id(7));
    patch.appearance = FieldUpdate::Clear;
    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(entity));
    let mut stale = ObjectPatch::new(source(region, 5), local_id(7));
    stale.appearance = FieldUpdate::Set(Appearance::Surface);
    let newer_materials = PbrMaterialAssignments::new(vec![(0, AssetId::new(Uuid::from_u128(32)))]).unwrap();
    stale.pbr_materials = Some(newer_materials.clone());
    stale.pbr_material_overrides = Some(snapshot(0, 0.9));
    let rejected = world.patch_object(stale).unwrap();
    assert_eq!(rejected.outcome, Updated(entity));
    assert!(rejected.rejections.iter().any(|rejection| rejection.group == FieldGroup::PbrMaterialOverrides));
    assert_eq!(world.object(entity).unwrap().appearance(), None);
    assert_eq!(world.object(entity).unwrap().pbr_materials(), Some(&newer_materials));
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides().unwrap().face(0).unwrap().roughness, Some(0.4));

    let mut clear = ObjectPatch::new(source(region, 11), local_id(7));
    clear.pbr_materials = Some(PbrMaterialAssignments::default());
    clear.pbr_material_overrides = Some(PbrMaterialOverrides::default());
    let cleared = world.patch_object(clear).unwrap();
    assert!(cleared.rejections.is_empty());
    assert_eq!(world.object(entity).unwrap().pbr_materials(), None);
    assert_eq!(world.object(entity).unwrap().pbr_material_overrides(), None);
}
