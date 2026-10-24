use std::num::{NonZeroU8, NonZeroU32};

use math::{Quat, Vec3};
use uuid::Uuid;

use crate::ApplyError::InvalidValue;
use crate::FieldUpdate;
use crate::Kind::{Avatar, Object};
use crate::OrderEvidence::Ordered;
use crate::ParentUpdate::{Local, Root};
use crate::{Appearance, AttachmentPoint, ObjectObservation, ObjectPatch, Pose, Region, RegionInfo, UpdateSource, World, WorldLimits};

fn limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 2,
        max_entities: 2,
        max_entities_per_region: 2,
        max_tombstones_per_region: 4,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: 3,
        max_journal_changes: 32,
        max_region_name_bytes: 8,
        max_recovery_targets: 4,
    }
}

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 48, max_shrink_iters: 128, ..Default::default() })]

    #[test]
    fn mutation_sequences_preserve_world_indexes(
        operations in proptest::collection::vec((0_u8..5, 0_u8..3, 0_u8..16, 0_u8..8, 1_u8..32, 0_u8..9, 0_u8..32), 1..128)
    ) {
        let regions = [Region::new(700), Region::new(701), Region::new(702)];
        let mut world_limits = limits();
        world_limits.max_region_registrations = regions.len();
        world_limits.max_entities = 16;
        world_limits.max_entities_per_region = 16;
        world_limits.max_tombstones_per_region = 64;
        world_limits.max_tombstones = 96;
        world_limits.max_tombstone_bytes_per_region = WorldLimits::tombstone_bytes_for_count(64);
        world_limits.max_tombstone_bytes = WorldLimits::tombstone_bytes_for_count(96);
        world_limits.max_journal_entries = 32;
        world_limits.max_journal_changes = 512;
        world_limits.max_recovery_targets = 16;
        let mut world = World::new(world_limits);

        for region in regions {
            let _ = world.adopt_region(region).expect("adopt generated region");
            proptest::prop_assert_eq!(world.check_invariants(), Ok(()));
        }

        for (ordinal, (operation, region_index, identity, slot, source_order, parent_slot, value)) in operations.into_iter().enumerate() {
            let region = regions[usize::from(region_index)];
            let local_id = NonZeroU32::new(u32::from(slot) + 1).expect("generated local ID is nonzero");
            let id = uuid(800 + u128::from(identity));
            let parent = match u32::from(parent_slot) {
                0 => Root,
                parent_local if parent_local == local_id.get() => Root,
                parent_local => Local(NonZeroU32::new(parent_local).expect("generated parent local ID is nonzero")),
            };
            let order = u64::from(source_order);
            let source = UpdateSource::new(region, u32::from(source_order), Ordered(order), ordinal as u32);
            let value = f32::from(value);

            match operation {
                0 => {
                    let kind = if identity.is_multiple_of(4) { Avatar } else { Object };
                    let mut observation = ObjectObservation::new(source, id, local_id, kind, parent);
                    observation.pose = Some(Pose { position: Some(Vec3::new(value, value * 0.5, 0.0)), rotation: Some(Quat::IDENTITY) });
                    observation.scale = Some(Vec3::splat(1.0 + value / 32.0));
                    let result = world.observe_object(observation);
                    proptest::prop_assert!(result.is_ok() || matches!(result, Err(InvalidValue { field: "parent_cycle" })), "unexpected observation result: {result:?}");
                }
                1 => {
                    let mut patch = ObjectPatch::new(source, local_id);
                    patch.parent = Some(parent);
                    patch.pose = Some(Pose { position: Some(Vec3::new(value * 0.25, 0.0, value)), rotation: Some(Quat::IDENTITY) });
                    patch.shape = if (value as u8).is_multiple_of(2) {
                        FieldUpdate::Set(crate::Shape::Prim(Default::default()))
                    } else {
                        FieldUpdate::Clear
                    };
                    patch.appearance = if (value as u8).is_multiple_of(3) {
                        FieldUpdate::Set(Appearance::Surface)
                    } else {
                        FieldUpdate::Keep
                    };
                    let _ = world.patch_object(patch).expect("apply valid patch");
                }
                2 => {
                    let _ = world.remove_object(source, local_id).expect("apply valid removal");
                }
                3 => {
                    let _ = world
                        .patch_region(region, RegionInfo { name: Some(format!("Region {region_index}")), ..RegionInfo::default() })
                        .expect("apply valid region metadata");
                }
                4 => {
                    let mut patch = ObjectPatch::new(source, local_id);
                    patch.attachment_point = if parent_slot == 0 {
                        FieldUpdate::Clear
                    } else {
                        FieldUpdate::Set(AttachmentPoint::new(NonZeroU8::new(parent_slot).expect("nonzero attachment point")))
                    };
                    let _ = world.patch_object(patch).expect("apply valid attachment update");
                }
                5.. => panic!("operation generator is bounded to 0..5"),
            }

            proptest::prop_assert_eq!(world.check_invariants(), Ok(()));
        }
    }
}
