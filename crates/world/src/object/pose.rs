use math::Quat;

use crate::World;
use crate::{
    CoordinateFrame, Entity, Kind, Parent, PoseError, PoseState, RegionInfo, UnresolvedReason, WorldPose, WorldTransform,
    WorldTransformState,
};

pub(crate) const MAX_PARENT_EDGES: usize = 16;

impl World {
    pub(crate) fn coordinate_frame(&self, entity: Entity) -> Result<CoordinateFrame, PoseError> {
        let mut current = entity;
        let mut edges = 0;

        loop {
            let Some(object) = self.store.object_record(current) else { return Ok(CoordinateFrame::Unknown) };
            match object.state.parent {
                Parent::Unknown => return Ok(CoordinateFrame::Unknown),
                Parent::Pending { address, expected_uuid } => return Ok(CoordinateFrame::PendingParent { address, expected_uuid }),
                Parent::Root => {
                    return if current == entity {
                        Ok(CoordinateFrame::RegionRelative { region: object.state.binding.address.region })
                    } else {
                        Ok(CoordinateFrame::LinksetRoot { root: current })
                    };
                }
                Parent::Resolved(parent) => {
                    if edges == MAX_PARENT_EDGES {
                        return Err(PoseError::TraversalLimitExceeded { max_edges: MAX_PARENT_EDGES });
                    }
                    edges += 1;
                    if self.store.object_record(parent).is_some_and(|parent| parent.kind == Kind::Avatar) {
                        return if current == entity {
                            Ok(CoordinateFrame::AttachmentPoint { avatar: parent, point: object.state.attachment_point })
                        } else {
                            Ok(CoordinateFrame::LinksetRoot { root: current })
                        };
                    }
                    current = parent;
                }
            }
        }
    }

    pub(crate) fn resolve_pose(&self, entity: Entity) -> Result<PoseState, PoseError> {
        let state = self.resolve_pose_without_provisional(entity)?;
        if let PoseState::Unresolved(reason) = state
            && let Some(object) = self.store.object_record(entity)
            && object.state.parent == Parent::Root
            && let Some(pose) = object.state.provisional_pose
        {
            return Ok(PoseState::Provisional { pose, reason });
        }
        Ok(state)
    }

    fn resolve_pose_without_provisional(&self, entity: Entity) -> Result<PoseState, PoseError> {
        let mut current = entity;
        let mut edges = 0;
        let mut accumulated = None;
        let mut can_resolve = true;
        let mut unresolved = None;

        loop {
            let Some(object) = self.store.object_record(current) else {
                record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                break;
            };
            let data = &object.state;
            let transform = object.state.transform;
            let local_pose = match (transform.position(), transform.rotation()) {
                (Some(position), Some(rotation)) => {
                    validate_rotation(rotation, current)?;
                    if !data.has_current_local_pose() {
                        record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                        can_resolve = false;
                        None
                    } else {
                        Some((position.as_dvec3(), rotation.as_dquat()))
                    }
                }
                _ => {
                    record_unresolved(&mut unresolved, UnresolvedReason::MissingLocalPose);
                    can_resolve = false;
                    None
                }
            };
            if let Some((position, rotation)) = local_pose
                && can_resolve
            {
                accumulated = Some(match accumulated {
                    Some((child_position, child_rotation)) => (position + rotation * child_position, rotation * child_rotation),
                    None => (position, rotation),
                });
            }

            match object.state.parent {
                Parent::Unknown => {
                    record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                    break;
                }
                Parent::Pending { .. } => {
                    return Ok(PoseState::Unresolved(UnresolvedReason::PendingParent));
                }
                Parent::Root => {
                    let region = object.state.binding.address.region;
                    let Some(origin) = self.region(region).and_then(RegionInfo::resolved_origin) else {
                        record_unresolved(&mut unresolved, UnresolvedReason::MissingRegionOrigin);
                        break;
                    };
                    if let Some((position, rotation)) = accumulated.filter(|_| can_resolve) {
                        return Ok(PoseState::Resolved(WorldPose {
                            position: origin + position,
                            rotation: rotation.as_quat().normalize(),
                        }));
                    }
                    break;
                }
                Parent::Resolved(parent) => {
                    let Some(parent_object) = self.store.object_record(parent) else {
                        record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                        break;
                    };
                    if object.state.binding.address.region != parent_object.state.binding.address.region {
                        record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                        can_resolve = false;
                        accumulated = None;
                    }
                    if parent_object.kind == Kind::Avatar {
                        record_unresolved(&mut unresolved, UnresolvedReason::MissingAttachmentJoint);
                        can_resolve = false;
                        accumulated = None;
                    }
                    if edges == MAX_PARENT_EDGES {
                        return Err(PoseError::TraversalLimitExceeded { max_edges: MAX_PARENT_EDGES });
                    }
                    edges += 1;
                    current = parent;
                }
            }
        }

        Ok(PoseState::Unresolved(unresolved.unwrap_or(UnresolvedReason::PoseNotValidForCurrentFrame)))
    }

    pub(crate) fn resolve_transform(&self, entity: Entity) -> Result<WorldTransformState, PoseError> {
        let pose = match self.resolve_pose(entity)? {
            PoseState::Resolved(pose) => pose,
            PoseState::Provisional { pose, reason } => return Ok(WorldTransformState::Provisional { pose, reason }),
            PoseState::Unresolved(reason) => return Ok(WorldTransformState::Unresolved(reason)),
        };
        let Some(object) = self.store.object_record(entity) else {
            return Ok(WorldTransformState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame));
        };
        match object.state.transform.scale() {
            Some(scale) => Ok(WorldTransformState::Resolved(WorldTransform { pose, scale })),
            None => Ok(WorldTransformState::MissingScale(pose)),
        }
    }
}

fn validate_rotation(rotation: Quat, entity: Entity) -> Result<(), PoseError> {
    let norm_squared = rotation.length_squared();
    if !rotation.is_finite() || !norm_squared.is_finite() || (norm_squared - 1.0).abs() > 8.0 * f32::EPSILON {
        Err(PoseError::InvalidRotation { entity })
    } else {
        Ok(())
    }
}

fn record_unresolved(current: &mut Option<UnresolvedReason>, next: UnresolvedReason) {
    let rank = |reason| match reason {
        UnresolvedReason::PendingParent => 0,
        UnresolvedReason::PoseNotValidForCurrentFrame => 1,
        UnresolvedReason::MissingLocalPose => 2,
        UnresolvedReason::MissingAttachmentJoint => 3,
        UnresolvedReason::MissingRegionOrigin => 4,
    };
    if current.is_none_or(|current| rank(next) < rank(current)) {
        *current = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::{
        ApplyOutcome::Introduced, Kind::Object, ObjectObservation, OrderEvidence::Ordered, ParentUpdate::Root, Pose, Region, RegionInfo,
        UpdateSource, WorldLimits,
    };
    use math::{DVec3, Vec3};
    use uuid::Uuid;

    #[test]
    fn corrupt_parent_cycle_is_bounded() {
        let region = Region::new(1);
        let limits = WorldLimits {
            max_region_registrations: 1,
            max_entities: 2,
            max_entities_per_region: 2,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: 4,
            max_journal_changes: 4,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        };
        let mut world = World::new(limits);
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));

        let mut introduce = |id, local_id, order| {
            let result = world
                .observe_object(ObjectObservation::new(
                    UpdateSource::new(region, order, Ordered(u64::from(order)), 0),
                    Uuid::from_u128(u128::from(id)),
                    NonZeroU32::new(local_id).expect("nonzero local ID"),
                    Object,
                    Root,
                ))
                .unwrap();
            let Introduced(entity) = result.outcome else { panic!("expected object introduction") };
            entity
        };
        let first = introduce(1_u32, 1, 1);
        let second = introduce(2_u32, 2, 2);
        #[cfg(feature = "invariants")]
        let first_uuid = world.object(first).expect("first object is live").id();
        #[cfg(feature = "invariants")]
        let second_uuid = world.object(second).expect("second object is live").id();

        world.state_mut(first).parent = Parent::Resolved(second);
        world.state_mut(second).parent = Parent::Resolved(first);

        assert_eq!(world.object(first).unwrap().world_pose(), Err(PoseError::TraversalLimitExceeded { max_edges: MAX_PARENT_EDGES }));
        #[cfg(feature = "invariants")]
        {
            world.children.entry(second).or_default().insert(first_uuid, first);
            world.children.entry(first).or_default().insert(second_uuid, second);
            assert_eq!(world.check_invariants(), Err("resolved parent graph contains a cycle"));
        }
    }

    #[test]
    fn invalid_canonical_rotation_is_reported() {
        let region = Region::new(2);
        let limits = WorldLimits {
            max_region_registrations: 1,
            max_entities: 1,
            max_entities_per_region: 1,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: 4,
            max_journal_changes: 4,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        };
        let mut world = World::new(limits);
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));
        assert!(matches!(
            world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap().outcome,
            crate::RegionOutcome::Patched { .. }
        ));
        let result = world
            .observe_object(ObjectObservation {
                pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
                ..ObjectObservation::new(
                    UpdateSource::new(region, 1, Ordered(1), 0),
                    Uuid::from_u128(3),
                    NonZeroU32::new(1).unwrap(),
                    Object,
                    Root,
                )
            })
            .unwrap();
        let Introduced(entity) = result.outcome else { panic!("expected object introduction") };
        world.store.record_mut(entity.0).state.transform.rotation = Some(Quat::from_xyzw(0.0, 0.0, 0.0, 2.0));

        assert_eq!(world.object(entity).unwrap().world_pose(), Err(PoseError::InvalidRotation { entity }));
    }
}
