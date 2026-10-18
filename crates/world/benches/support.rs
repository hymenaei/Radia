use std::num::NonZeroU32;

use base::UUID;
use math::{Quat, Vec3};
use world::{
    ApplyOutcome::Introduced,
    Kind,
    Kind::{Avatar, Object},
    ObjectObservation,
    OrderEvidence::Ordered,
    ParentUpdate::Root,
    Pose, Region, UpdateSource, World, WorldLimits,
};

pub(super) const PER_REGION: usize = 10_000;
pub(super) const SCENE_SIZES: &[usize] = &[1_000, 10_000, 65_536];
pub(super) const LOAD_SIZES: &[usize] = &[40_000, 65_536];
pub(super) const INITIAL_ORDER: u64 = 100_000;

#[derive(Clone, Copy)]
pub(super) struct Row {
    pub(super) id: UUID,
    pub(super) region: Region,
    pub(super) local_id: NonZeroU32,
    pub(super) order: u64,
    pub(super) kind: Kind,
}

pub(super) struct Scene {
    pub(super) world: World,
    pub(super) rows: Vec<Row>,
    pub(super) regions: Vec<Region>,
}

pub(super) fn size_name(count: usize) -> &'static str {
    match count {
        1_000 => "1k",
        10_000 => "10k",
        40_000 => "40k",
        65_536 => "65k",
        _ => panic!("unexpected benchmark scene size"),
    }
}

pub(super) fn limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 8,
        max_entities: 65_536,
        max_entities_per_region: PER_REGION,
        max_tombstones_per_region: 4_096,
        max_tombstones: 16_384,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(4_096),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16_384),
        max_journal_entries: 4_096,
        max_journal_changes: 65_536,
        max_region_name_bytes: 256,
        max_recovery_targets: 256,
    }
}

pub(super) fn rows(count: usize, avatar_count: usize) -> (Vec<Row>, Vec<Region>) {
    assert!(avatar_count <= count);
    let region_count = count.div_ceil(PER_REGION).max(1);
    let regions = (0..region_count).map(|index| Region::new(index as u64 + 1)).collect::<Vec<_>>();
    let rows = (0..count)
        .map(|index| {
            let region_index = index / PER_REGION;
            let local_id = NonZeroU32::new((index % PER_REGION + 1) as u32).expect("local ID is nonzero");
            Row {
                id: UUID::from_u128(index as u128 + 1),
                region: regions[region_index],
                local_id,
                order: INITIAL_ORDER + u64::from(local_id.get()),
                kind: if index < avatar_count { Avatar } else { Object },
            }
        })
        .collect();
    (rows, regions)
}

pub(super) fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, u32::try_from(order).expect("benchmark source fits u32"), Ordered(order), 0)
}

pub(super) fn observation(row: Row) -> ObjectObservation {
    observation_with_pose(row, Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) })
}

pub(super) fn observation_with_pose(row: Row, pose: Pose) -> ObjectObservation {
    let mut observation = ObjectObservation::new(source(row.region, row.order), row.id, row.local_id, row.kind, Root);
    observation.pose = Some(pose);
    observation.scale = Some(Vec3::ONE);
    observation
}

pub(super) fn empty_scene(count: usize, destination: bool) -> Scene {
    let (rows, mut regions) = rows(count, 0);
    if destination {
        regions.push(Region::new(regions.len() as u64 + 1));
    }
    let mut world = World::new(limits());
    for region in &regions {
        assert!(matches!(world.adopt_region(*region).expect("region adoption").outcome, world::RegionOutcome::Adopted { .. }));
    }
    Scene { world, rows, regions }
}

pub(super) fn populated_scene(count: usize, agent: bool, destination: bool) -> Scene {
    populated_scene_with_avatars(count, usize::from(agent), destination)
}

pub(super) fn populated_scene_with_avatars(count: usize, avatar_count: usize, destination: bool) -> Scene {
    let (rows, mut regions) = rows(count, avatar_count);
    if destination {
        regions.push(Region::new(regions.len() as u64 + 1));
    }
    let mut world = World::new(limits());
    for region in &regions {
        assert!(matches!(world.adopt_region(*region).expect("region adoption").outcome, world::RegionOutcome::Adopted { .. }));
    }
    if avatar_count > 0 {
        world.set_self_agent(rows[0].id).expect("agent identity");
    }
    for row in &rows {
        let applied = world.observe_object(observation(*row)).expect("object introduction");
        assert!(matches!(applied.outcome, Introduced(_)));
    }
    assert_eq!(world.object_count(), count);
    Scene { world, rows, regions }
}
