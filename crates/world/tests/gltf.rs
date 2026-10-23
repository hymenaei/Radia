use std::num::NonZeroU32;

use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance,
    ApplyOutcome::{Introduced, Unchanged, Updated},
    FieldGroup, FieldRejectionReason, FieldUpdate, GltfMaterialOverride, GltfMaterialOverrides, Kind, ObjectObservation, ObjectPatch,
    OrderEvidence, ParentUpdate, Region, RenderMaterials, UpdateAuthority, UpdateSource, World, WorldLimits,
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

fn snapshot(face: u8, roughness: f32) -> GltfMaterialOverrides {
    GltfMaterialOverrides::new(vec![(face, GltfMaterialOverride { roughness: Some(roughness), ..GltfMaterialOverride::default() })])
        .expect("valid override snapshot")
}

fn object(source: UpdateSource, id: u128, local_id_value: u32, with_surface: bool) -> ObjectObservation {
    ObjectObservation {
        appearance: if with_surface { FieldUpdate::Set(Appearance::Surface) } else { FieldUpdate::Keep },
        render_materials: Some(RenderMaterials::new(vec![(2, AssetId::new(Uuid::from_u128(29)))]).expect("valid PBR bindings")),
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
    first.gltf_material_overrides = Some(snapshot(1, 0.2));
    assert_eq!(world.patch_object(first).unwrap().outcome, Updated(entity));
    assert_eq!(world.object(entity).unwrap().gltf_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.2));

    let mut replacement = ObjectPatch::new(source(region, 3), local_id(7));
    replacement.gltf_material_overrides = Some(snapshot(4, 0.8));
    assert_eq!(world.patch_object(replacement).unwrap().outcome, Updated(entity));
    {
        let object = world.object(entity).unwrap();
        let overrides = object.gltf_material_overrides().unwrap();
        assert_eq!(overrides.face(1), None);
        assert_eq!(overrides.face(4).unwrap().roughness, Some(0.8));
        assert_eq!(object.render_materials().unwrap().material(2).unwrap().uuid(), Uuid::from_u128(29));
    }

    let mut stale = ObjectPatch::new(source(region, 2), local_id(7));
    stale.gltf_material_overrides = Some(snapshot(8, 0.1));
    assert_eq!(world.patch_object(stale).unwrap().outcome, Unchanged);
    assert_eq!(world.object(entity).unwrap().gltf_material_overrides().unwrap().face(4).unwrap().roughness, Some(0.8));

    let mut clear = ObjectPatch::new(source(region, 4), local_id(7));
    clear.gltf_material_overrides = Some(GltfMaterialOverrides::default());
    assert_eq!(world.patch_object(clear).unwrap().outcome, Updated(entity));
    let object = world.object(entity).unwrap();
    assert_eq!(object.gltf_material_overrides(), None);
    assert_eq!(object.render_materials().unwrap().material(2).unwrap().uuid(), Uuid::from_u128(29));
}

#[test]
fn override_is_retained_before_base_binding() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, true)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };

    let mut override_update = ObjectPatch::new(source(region, 2), local_id(7));
    override_update.gltf_material_overrides = Some(snapshot(1, 0.4));
    assert_eq!(world.patch_object(override_update).unwrap().outcome, Updated(entity));

    {
        let current = world.object(entity).unwrap();
        assert_eq!(current.render_materials().unwrap().material(1), None);
        assert_eq!(current.gltf_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.4));
    }

    let mut material_update = ObjectPatch::new(source(region, 3), local_id(7));
    material_update.render_materials = Some(RenderMaterials::new(vec![(1, AssetId::new(Uuid::from_u128(31)))]).expect("valid binding"));
    assert_eq!(world.patch_object(material_update).unwrap().outcome, Updated(entity));

    let current = world.object(entity).unwrap();
    assert_eq!(current.render_materials().unwrap().material(1).unwrap().uuid(), Uuid::from_u128(31));
    assert_eq!(current.gltf_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.4));
}

#[test]
fn rejects_overrides_without_surface_appearance() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, false)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    let mut patch = ObjectPatch::new(source(region, 2), local_id(7));
    patch.gltf_material_overrides = Some(snapshot(1, 0.2));

    let applied = world.patch_object(patch).unwrap();

    assert!(applied.rejections.iter().any(|rejection| {
        rejection.group == FieldGroup::GltfMaterialOverrides
            && rejection.reason == FieldRejectionReason::InvalidValue { field: "gltf_material_overrides" }
    }));
    assert_eq!(world.object(entity).unwrap().gltf_material_overrides(), None);
}

#[test]
fn untracked_snapshot_cannot_replace_tracked_data() {
    let region = Region::new(11);
    let mut world = with_region(region);
    let introduced = world.observe_object(object(source(region, 1), 17, 7, true)).unwrap();
    let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
    let mut tracked = ObjectPatch::new(source(region, 2), local_id(7));
    tracked.gltf_material_overrides = Some(snapshot(1, 0.4));
    assert_eq!(world.patch_object(tracked).unwrap().outcome, Updated(entity));

    let untracked_source = source(region, 3).with_authority(UpdateAuthority::Untracked);
    let mut untracked = ObjectPatch::new(untracked_source, local_id(7));
    untracked.gltf_material_overrides = Some(snapshot(1, 0.9));
    let applied = world.patch_object(untracked).unwrap();

    assert!(applied.rejections.iter().any(|rejection| {
        rejection.group == FieldGroup::GltfMaterialOverrides && rejection.reason == FieldRejectionReason::UntrustedSource
    }));
    assert_eq!(world.object(entity).unwrap().gltf_material_overrides().unwrap().face(1).unwrap().roughness, Some(0.4));
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
    patch.gltf_material_overrides = Some(snapshot(3, 0.7));

    assert_eq!(world.patch_object(patch).unwrap().outcome, Updated(second_entity));

    assert_eq!(world.object(first_entity).unwrap().gltf_material_overrides(), None);
    assert_eq!(world.object(second_entity).unwrap().gltf_material_overrides().unwrap().face(3).unwrap().roughness, Some(0.7));
}
