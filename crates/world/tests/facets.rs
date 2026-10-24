use std::num::NonZeroU32;

use proptest::prelude::*;
use utils::AssetId;
use uuid::Uuid;
use world::{
    Appearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence, ParentUpdate, PbrMaterialAssignments, Region, Shape,
    UpdateAuthority, UpdateSource, World, WorldLimits,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelStamp {
    authority: UpdateAuthority,
    order: u64,
    block: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct FacetModel<T> {
    value: Option<T>,
    fence: Option<ModelStamp>,
}

impl<T> Default for FacetModel<T> {
    fn default() -> Self {
        Self { value: None, fence: None }
    }
}

impl<T> FacetModel<T> {
    fn apply(&mut self, stamp: ModelStamp, value: Option<T>) {
        if newer(stamp, self.fence) {
            self.value = value;
            self.fence = Some(stamp);
        }
    }
}

fn newer(stamp: ModelStamp, fence: Option<ModelStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ => (stamp.order, stamp.block) > (fence.order, fence.block),
    })
}

fn world() -> World {
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: 1,
        max_entities_per_region: 1,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 256,
        max_journal_changes: 512,
        max_region_name_bytes: 8,
        max_recovery_targets: 1,
    });
    assert!(matches!(world.adopt_region(Region::new(900)).unwrap().outcome, world::RegionOutcome::Adopted { .. }));
    world
}

fn source(region: Region, authority: UpdateAuthority, order: u64, block: u32) -> UpdateSource {
    UpdateSource::new(region, order as u32, OrderEvidence::Ordered(order), block).with_authority(authority)
}

fn shape(value: u128) -> Shape {
    Shape::Mesh { asset_id: AssetId::new(Uuid::from_u128(value)), flags: Default::default() }
}

fn materials(value: u128) -> PbrMaterialAssignments {
    PbrMaterialAssignments::new(vec![(0, AssetId::new(Uuid::from_u128(value)))]).unwrap()
}

proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 64,
        max_shrink_iters: 128,
        ..proptest::test_runner::Config::default()
    })]

    #[test]
    fn facet_fences_match_model(
        operations in prop::collection::vec((0_u8..2, 0_u8..3, 0_u8..2, 0_u64..24, 0_u32..8, 0_u8..2, 1_u128..128), 1..80)
    ) {
        let region = Region::new(900);
        let id = Uuid::from_u128(900);
        let local_id = NonZeroU32::new(1).unwrap();
        let mut world = world();
        let initial = ModelStamp { authority: UpdateAuthority::Tracked, order: 0, block: 0 };
        let introduced = world.observe_object(ObjectObservation::new(
            source(region, initial.authority, initial.order, initial.block), id, local_id, Kind::Object, ParentUpdate::Root,
        )).unwrap();
        let world::ApplyOutcome::Introduced(entity) = introduced.outcome else { panic!("initial object was not introduced") };

        let mut shapes = FacetModel::<Shape>::default();
        let mut appearances = FacetModel::<Appearance>::default();
        let mut material_ids = FacetModel::<u128>::default();
        let mut binding_fence = Some((initial.order, initial.block));
        let mut parent_fence = Some(initial);

        for (index, (observation, facet, tracked, order, block, clear, value)) in operations.into_iter().enumerate() {
            let authority = if tracked == 0 { UpdateAuthority::Tracked } else { UpdateAuthority::Untracked };
            let stamp = ModelStamp { authority, order, block };
            let source = source(region, authority, order, block);
            let shaped = shape(10_000 + value);
            let appearance = if value.is_multiple_of(2) { Appearance::Surface } else { Appearance::Avatar };
            let material_id = 20_000 + value;
            let material = materials(material_id);
            let value_present = clear == 0;
            let should_stale = observation == 0 &&
                !binding_fence.is_none_or(|fence| (order, block) > fence) &&
                !newer(stamp, parent_fence) &&
                match facet {
                    0 => !newer(stamp, shapes.fence),
                    1 => !newer(stamp, appearances.fence),
                    _ => !newer(stamp, material_ids.fence),
                };

            if observation == 0 {
                let mut update = ObjectObservation::new(source, id, local_id, Kind::Object, ParentUpdate::Root);
                match facet {
                    0 => update.shape = if value_present { FieldUpdate::Set(shaped) } else { FieldUpdate::Clear },
                    1 => update.appearance = if value_present { FieldUpdate::Set(appearance) } else { FieldUpdate::Clear },
                    _ => update.pbr_materials = Some(if value_present { material } else { PbrMaterialAssignments::default() }),
                }
                let result = world.observe_object(update).unwrap();
                prop_assert_eq!(
                    matches!(result.outcome, world::ApplyOutcome::IgnoredStale), should_stale,
                    "observation staleness differs at generated step {}", index
                );
                if !should_stale {
                    match facet {
                        0 => shapes.apply(stamp, value_present.then_some(shaped)),
                        1 => appearances.apply(stamp, value_present.then_some(appearance)),
                        _ => material_ids.apply(stamp, value_present.then_some(material_id)),
                    }
                    if newer(stamp, parent_fence) {
                        parent_fence = Some(stamp);
                    }
                    if authority == UpdateAuthority::Tracked {
                        binding_fence = Some(binding_fence.map_or((order, block), |fence| fence.max((order, block))));
                    }
                }
            } else {
                let mut update = ObjectPatch::new(source, local_id);
                match facet {
                    0 => update.shape = if value_present { FieldUpdate::Set(shaped) } else { FieldUpdate::Clear },
                    1 => update.appearance = if value_present { FieldUpdate::Set(appearance) } else { FieldUpdate::Clear },
                    _ => update.pbr_materials = Some(if value_present { material } else { PbrMaterialAssignments::default() }),
                }
                let result = world.patch_object(update).unwrap();
                prop_assert!(
                    result.outcome == world::ApplyOutcome::Unchanged
                        || matches!(result.outcome, world::ApplyOutcome::Updated(updated) if updated == entity),
                    "patch produced an unexpected outcome at step {}", index
                );
                match facet {
                    0 => shapes.apply(stamp, value_present.then_some(shaped)),
                    1 => appearances.apply(stamp, value_present.then_some(appearance)),
                    _ => material_ids.apply(stamp, value_present.then_some(material_id)),
                }
                if authority == UpdateAuthority::Tracked {
                    binding_fence = Some(binding_fence.map_or((order, block), |fence| fence.max((order, block))));
                }
            }

            let object = world.object(entity).unwrap();
            prop_assert_eq!(object.shape(), shapes.value, "shape value differs after step {}", index);
            prop_assert_eq!(object.appearance(), appearances.value, "appearance value differs after step {}", index);
            let actual_material = object.pbr_materials().and_then(|value| value.material(0)).map(|value| value.uuid().as_u128());
            prop_assert_eq!(actual_material, material_ids.value, "PBR value differs after step {}", index);
        }
    }
}
