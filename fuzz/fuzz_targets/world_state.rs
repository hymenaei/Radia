#![no_main]

use std::{f32::consts::TAU, num::NonZeroU32};

use base::UUID;
use libfuzzer_sys::fuzz_target;
use math::{Quat, Vec3};
use world::{
    AgentHandoff, ApplyError::RegionRetired, Kind, LinearMotion, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate,
    Pose, Region, RegionHandle, RegionInfo, UpdateSource, World, WorldLimits,
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
    UpdateSource::new(region, step as u32, Ordered(order), u32::from(bytes[30]))
}

fn uuid(bytes: &[u8]) -> UUID {
    let mut value = [0; 16];
    value.copy_from_slice(&bytes[4..20]);
    let value = u128::from_le_bytes(value);
    UUID::from_u128(value.max(1))
}

fn local_id(bytes: &[u8]) -> NonZeroU32 {
    let value = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]).max(1);
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

fn parent(bytes: &[u8], local_id: NonZeroU32) -> ParentUpdate {
    let value = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    if value == 0 { ParentUpdate::Root } else { ParentUpdate::Local(NonZeroU32::new(value).unwrap_or(local_id)) }
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

    for (step, bytes) in data.chunks_exact(32).take(MAX_OPERATIONS).enumerate() {
        let region_index = usize::from(bytes[1]) % regions.len();
        let region = regions[region_index];
        let source = source(region, step + 1, bytes);
        let local_id = local_id(bytes);
        let id = uuid(bytes);
        let operation = bytes[0] % 7;

        match operation {
            0 => {
                let kind = match bytes[2] % 4 {
                    0 => Kind::Object,
                    1 => Kind::Avatar,
                    value => Kind::Unknown(value),
                };
                let mut observation = ObjectObservation::new(source, id, local_id, kind, parent(bytes, local_id));
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
                    patch.parent = Some(parent(bytes, local_id))
                }
                if bytes[3] & 8 != 0 {
                    patch.scale = Some(Vec3::new(1.0, 1.0, 1.0))
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
                    let _ = world.patch_properties(source, target, id, Default::default());
                }
            }
            4 => {
                let handle = u64::from_le_bytes(bytes[8..16].try_into().expect("fixed handle bytes"));
                let _ = world.patch_region(region, RegionInfo { handle: Some(RegionHandle::new(handle)), ..RegionInfo::default() });
            }
            5 => {
                let from = world.entity(AGENT_ID).and_then(|entity| world.object(entity)).map(|agent| agent.address().region());
                if let Some(from) = from {
                    if let Some(destination) = regions.iter().copied().find(|region| *region != from) {
                        let handoff = AgentHandoff {
                            source: UpdateSource::new(destination, step as u32, Ordered(step as u64 + 1), 0),
                            local_id,
                            parent: parent(bytes, local_id),
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
    }
});
