#![no_main]

use std::{f32::consts::TAU, num::NonZeroU32};

use base::UUID;
use libfuzzer_sys::fuzz_target;
use math::{Quat, Vec3};
use world::{
    AgentHandoff, Appearance,
    ApplyError::RegionRetired,
    FieldUpdate, Kind, LinearMotion, ObjectObservation, ObjectPatch,
    OrderEvidence::{Ambiguous, Degraded, Ordered},
    ParentUpdate, Pose, PropertiesPatch, Region, RegionHandle, RegionInfo, Shape, UpdateAuthority, UpdateSource, World, WorldLimits,
    WorldReadError::ResyncRequired,
};

const MAX_OPERATIONS: usize = 256;
const AGENT_ID: UUID = UUID::from_u128(1);

fn limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: MAX_OPERATIONS * 2,
        max_entities: MAX_OPERATIONS,
        max_entities_per_region: MAX_OPERATIONS,
        max_tombstones_per_region: MAX_OPERATIONS,
        max_tombstones: MAX_OPERATIONS,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(MAX_OPERATIONS),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(MAX_OPERATIONS),
        max_journal_entries: 32,
        max_journal_changes: MAX_OPERATIONS,
        max_region_name_bytes: 16,
        max_recovery_targets: 8,
    }
}

fn source(region: Region, step: usize, bytes: &[u8]) -> UpdateSource {
    let order = u64::from(u16::from_le_bytes([bytes[28], bytes[29]]));
    let evidence = match bytes[31] & 15 {
        1 => Ambiguous,
        2 => Degraded,
        _ => Ordered(order),
    };
    let source = UpdateSource::new(region, step as u32, evidence, u32::from(bytes[30]));
    if bytes[31] & 15 == 3 { source.with_authority(UpdateAuthority::Untracked) } else { source }
}

fn uuid(bytes: &[u8]) -> UUID {
    if bytes[31] & 128 == 0 {
        return UUID::from_u128(u128::from(bytes[4] % 16) + 2);
    }
    let value = u128::from_le_bytes(bytes[4..20].try_into().expect("fixed UUID bytes"));
    UUID::from_u128(value.max(1))
}

fn local_id(bytes: &[u8]) -> NonZeroU32 {
    let value = if bytes[31] & 128 == 0 {
        u32::from(bytes[20] % 16) + 1
    } else {
        u32::from_le_bytes(bytes[20..24].try_into().expect("fixed local ID bytes")).max(1)
    };
    NonZeroU32::new(value).expect("local ID is nonzero")
}

fn pose(bytes: &[u8]) -> Pose {
    let position = Vec3::new(
        f32::from(i16::from_le_bytes([bytes[24], bytes[25]])) / 64.0,
        f32::from(i16::from_le_bytes([bytes[26], bytes[27]])) / 64.0,
        f32::from(bytes[28]) / 16.0,
    );
    let angle = f32::from(bytes[29]) * (TAU / 255.0);
    Pose { position: Some(position), rotation: Some(Quat::from_rotation_y(angle)) }
}

fn parent(bytes: &[u8]) -> ParentUpdate {
    let value = if bytes[31] & 128 == 0 {
        u32::from(bytes[24] % 17)
    } else {
        u32::from_le_bytes(bytes[24..28].try_into().expect("fixed parent ID bytes"))
    };
    if value == 0 { ParentUpdate::Root } else { ParentUpdate::Local(NonZeroU32::new(value).expect("parent local ID is nonzero")) }
}

fn assert_world(world: &World) {
    world.check_invariants().expect("World storage and indexes remain consistent");
}

fuzz_target!(|data: &[u8]| {
    let mut world = World::new(limits());
    let mut next_region = 1_u64;
    let first = Region::new(next_region);
    next_region += 1;
    let second = Region::new(next_region);
    next_region += 1;
    let _ = world.adopt_region(first);
    let _ = world.adopt_region(second);
    let mut regions = vec![first, second];
    let _ = world.set_self_agent(AGENT_ID);
    let mut initial = ObjectObservation::new(
        UpdateSource::new(first, 0, Ordered(0), 0),
        AGENT_ID,
        NonZeroU32::new(1).expect("nonzero local ID"),
        Kind::Avatar,
        ParentUpdate::Root,
    );
    initial.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
    initial.linear_motion = Some(LinearMotion::default());
    let _ = world.observe_object(initial);
    assert_world(&world);
    let barrier = world.read_barrier();
    let mut changes = barrier.changes;
    let mut completed_revision = barrier.revision;

    for (step, bytes) in data.chunks_exact(32).take(MAX_OPERATIONS).enumerate() {
        let region_index = usize::from(bytes[1]) % regions.len();
        let region = regions[region_index];
        let source = source(region, step + 1, bytes);
        let local_id = local_id(bytes);
        let id = uuid(bytes);
        let operation = bytes[0] % 7;

        match operation {
            0 => {
                let kind = match (if bytes[31] & 128 == 0 { bytes[4] } else { bytes[2] }) % 4 {
                    0 => Kind::Object,
                    1 => Kind::Avatar,
                    value => Kind::Unknown(value),
                };
                let mut observation = ObjectObservation::new(source, id, local_id, kind, parent(bytes));
                observation.pose = (bytes[3] & 1 != 0).then(|| pose(bytes));
                observation.linear_motion = (bytes[3] & 2 != 0).then_some(LinearMotion {
                    velocity: Some(Vec3::new(f32::from(bytes[24]), f32::from(bytes[25]), f32::from(bytes[26]))),
                    acceleration: Some(Vec3::new(f32::from(bytes[27]), f32::from(bytes[28]), f32::from(bytes[29]))),
                });
                observation.scale = (bytes[3] & 4 != 0).then_some(Vec3::new(1.0, 1.0, 1.0));
                let _ = world.observe_object(observation);
            }
            1 => {
                let mut patch = ObjectPatch::new(source, local_id);
                if bytes[3] & 1 != 0 {
                    patch.pose = Some(pose(bytes))
                }
                if bytes[3] & 2 != 0 {
                    patch.linear_motion.velocity = Some(Vec3::new(f32::from(bytes[24]), f32::from(bytes[25]), f32::from(bytes[26])));
                    patch.linear_motion.acceleration = Some(Vec3::new(f32::from(bytes[27]), f32::from(bytes[28]), f32::from(bytes[29])));
                }
                if bytes[3] & 4 != 0 {
                    patch.parent = Some(parent(bytes))
                }
                if bytes[3] & 8 != 0 {
                    patch.scale = Some(Vec3::new(1.0, 1.0, 1.0))
                }
                if bytes[3] & 16 != 0 {
                    patch.shape = if bytes[5] & 1 == 0 { FieldUpdate::Clear } else { FieldUpdate::Set(Shape::Prim(Default::default())) };
                }
                if bytes[3] & 32 != 0 {
                    patch.appearance = if bytes[5] & 2 == 0 { FieldUpdate::Clear } else { FieldUpdate::Set(Appearance::Surface) };
                }
                let _ = world.patch_object(patch);
            }
            2 => {
                let _ = world.remove_object(source, local_id);
            }
            3 => {
                if let Some(entity) = world.entity(id)
                    && let Some(target) = world.object_binding_target(entity)
                {
                    let patch = PropertiesPatch {
                        owner_id: (bytes[3] & 1 != 0).then(|| UUID::from_u128(u128::from(bytes[8]))),
                        creator_id: (bytes[3] & 2 != 0).then(|| UUID::from_u128(u128::from(bytes[9]))),
                    };
                    let _ = world.patch_properties(source, target, id, patch);
                }
            }
            4 => {
                let handle = u64::from_le_bytes(bytes[8..16].try_into().expect("fixed handle bytes"));
                if bytes[3] & 1 == 0 {
                    let _ = world.patch_region(region, RegionInfo { handle: Some(RegionHandle::new(handle)), ..RegionInfo::default() });
                } else {
                    let name = String::from_utf8_lossy(&bytes[24..32]).into_owned();
                    let _ = world.observe_region_metadata(source, RegionInfo { name: Some(name), ..RegionInfo::default() });
                }
            }
            5 => {
                let from = world.entity(AGENT_ID).and_then(|entity| world.object(entity)).map(|agent| agent.address().region());
                if let Some(from) = from {
                    if let Some(destination) = regions.iter().copied().find(|region| *region != from) {
                        let handoff = AgentHandoff {
                            source: UpdateSource::new(destination, step as u32, Ordered(step as u64 + 1), 0),
                            local_id,
                            parent: parent(bytes),
                            pose: Some(pose(bytes)),
                            linear_motion: None,
                            angular_velocity: None,
                            scale: None,
                        };
                        let _ = world.handoff_agent(from, handoff);
                    }
                }
            }
            _ if regions.len() > 1 => {
                if world.retire_region(region).is_ok() {
                    regions.remove(region_index);
                    assert_eq!(world.adopt_region(region), Err(RegionRetired));
                    let replacement = Region::new(next_region);
                    next_region += 1;
                    if world.adopt_region(replacement).is_ok() {
                        regions.push(replacement);
                    }
                }
            }
            _ => {}
        }

        assert_world(&world);
        if bytes[30] & 1 == 0 {
            let limit = usize::from((bytes[30] >> 1) % 8) + 1;
            match world.change_page(changes, limit) {
                Ok(page) => {
                    assert_eq!(page.observed_revision, world.revision());
                    assert!(page.completed_revision >= completed_revision);
                    assert!(page.completed_revision <= page.observed_revision);
                    assert!(page.records.len() <= limit);
                    assert!(page.records.iter().all(|record| record.revision > completed_revision));
                    assert!(page.records.windows(2).all(|pair| pair[0].revision <= pair[1].revision));
                    if page.has_more {
                        assert_eq!(page.records.len(), limit);
                    } else {
                        assert_eq!(page.completed_revision, page.observed_revision);
                    }
                    changes = page.cursor;
                    completed_revision = page.completed_revision;
                }
                Err(ResyncRequired) => {
                    let barrier = world.read_barrier();
                    changes = barrier.changes;
                    completed_revision = barrier.revision;
                }
                Err(error) => panic!("valid change cursor failed: {error}"),
            }
        }
    }
});
