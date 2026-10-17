use math::{DQuat, DVec3, Quat};

use super::{
    CoordinateFrame, Entity, Kind, Parent, PoseError, PoseState, Region, UnresolvedReason, World, WorldPose, WorldTransform,
    WorldTransformState,
};

pub(super) const MAX_PARENT_EDGES: usize = 16;

impl World {
    pub(super) fn coordinate_frame(&self, entity: Entity) -> Result<CoordinateFrame, PoseError> {
        self.check_parent_cycle(entity)?;
        if self.object(entity).is_none() {
            return Ok(CoordinateFrame::Unknown);
        }
        let mut current = entity;
        let mut edges = 0;

        loop {
            let Some(object) = self.object(current) else { return Ok(CoordinateFrame::Unknown) };
            match object.parent() {
                Parent::Unknown => return Ok(CoordinateFrame::Unknown),
                Parent::Pending { address, expected_uuid } => return Ok(CoordinateFrame::PendingParent { address, expected_uuid }),
                Parent::Root => {
                    return if current == entity {
                        Ok(CoordinateFrame::RegionRelative { region: object.address().region() })
                    } else {
                        Ok(CoordinateFrame::LinksetRoot { root: current })
                    };
                }
                Parent::Resolved(parent) => {
                    if edges == MAX_PARENT_EDGES {
                        return Err(PoseError::TraversalLimitExceeded { max_edges: MAX_PARENT_EDGES });
                    }
                    edges += 1;
                    if self.object(parent).is_some_and(|parent| parent.kind() == Kind::Avatar) {
                        return if current == entity {
                            Ok(CoordinateFrame::AttachmentPoint { avatar: parent, point: object.attachment_point() })
                        } else {
                            Ok(CoordinateFrame::LinksetRoot { root: current })
                        };
                    }
                    current = parent;
                }
            }
        }
    }

    pub(super) fn resolve_pose(&self, entity: Entity) -> Result<PoseState, PoseError> {
        let state = self.resolve_pose_without_provisional(entity)?;
        if let PoseState::Unresolved(reason) = state
            && let Some(object) = self.object(entity)
            && object.parent() == Parent::Root
            && let Some(pose) = object.data.provisional_pose
        {
            return Ok(PoseState::Provisional { pose, reason });
        }
        Ok(state)
    }

    fn resolve_pose_without_provisional(&self, entity: Entity) -> Result<PoseState, PoseError> {
        self.check_parent_cycle(entity)?;
        let Some(_) = self.object(entity) else { return Ok(PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame)) };

        let mut current = entity;
        let mut edges = 0;
        let mut accumulated = None;
        let mut can_resolve = true;
        let mut unresolved = None;

        loop {
            let Some(object) = self.object(current) else {
                record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                break;
            };
            let data = &*object.data;
            let transform = object.transform();
            let local_pose = match (transform.position(), transform.rotation()) {
                (Some(position), Some(rotation)) => {
                    validate_rotation(rotation, current)?;
                    if !data.has_current_local_pose(transform) {
                        record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                        can_resolve = false;
                        None
                    } else {
                        Some((DVec3::new(position.x as f64, position.y as f64, position.z as f64), to_double_rotation(rotation)))
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

            match object.parent() {
                Parent::Unknown => {
                    record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                    break;
                }
                Parent::Pending { .. } => {
                    return Ok(PoseState::Unresolved(UnresolvedReason::PendingParent));
                }
                Parent::Root => {
                    let region = object.address().region();
                    let Some(origin) = self.region_origin(region) else {
                        record_unresolved(&mut unresolved, UnresolvedReason::MissingRegionOrigin);
                        break;
                    };
                    if let Some((position, rotation)) = accumulated.filter(|_| can_resolve) {
                        return Ok(PoseState::Resolved(WorldPose { position: origin + position, rotation: to_float_rotation(rotation) }));
                    }
                    break;
                }
                Parent::Resolved(parent) => {
                    let Some(parent_object) = self.object(parent) else {
                        record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                        break;
                    };
                    if object.address().region() != parent_object.address().region() {
                        record_unresolved(&mut unresolved, UnresolvedReason::PoseNotValidForCurrentFrame);
                        can_resolve = false;
                        accumulated = None;
                    }
                    if parent_object.kind() == Kind::Avatar {
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

    pub(super) fn resolve_transform(&self, entity: Entity) -> Result<WorldTransformState, PoseError> {
        let pose = self.resolve_pose(entity)?;
        let PoseState::Resolved(pose) = pose else { return Ok(WorldTransformState::Unresolved(pose)) };
        let Some(object) = self.object(entity) else {
            return Ok(WorldTransformState::Unresolved(PoseState::Unresolved(UnresolvedReason::PoseNotValidForCurrentFrame)));
        };
        match object.scale() {
            Some(scale) => Ok(WorldTransformState::Resolved(WorldTransform { pose, scale })),
            None => Ok(WorldTransformState::MissingScale(pose)),
        }
    }

    fn region_origin(&self, region: Region) -> Option<DVec3> {
        let info = self.region(region)?;
        info.origin.or_else(|| info.handle.map(|handle| handle.origin()))
    }

    fn check_parent_cycle(&self, entity: Entity) -> Result<(), PoseError> {
        let mut slow = Some(entity);
        let mut fast = Some(entity);
        for _ in 0..=MAX_PARENT_EDGES {
            slow = slow.and_then(|current| self.resolved_parent(current));
            fast = fast.and_then(|current| self.resolved_parent(current)).and_then(|current| self.resolved_parent(current));
            if let (Some(slow), Some(fast)) = (slow, fast)
                && slow == fast
            {
                return Err(PoseError::ParentCycle);
            }
            if slow.is_none() || fast.is_none() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn resolved_parent(&self, entity: Entity) -> Option<Entity> {
        self.object(entity).and_then(|object| match object.parent() {
            Parent::Resolved(parent) => Some(parent),
            Parent::Unknown | Parent::Root | Parent::Pending { .. } => None,
        })
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

fn to_double_rotation(rotation: Quat) -> DQuat {
    DQuat::from_xyzw(rotation.x as f64, rotation.y as f64, rotation.z as f64, rotation.w as f64)
}

fn to_float_rotation(rotation: DQuat) -> Quat {
    Quat::from_xyzw(rotation.x as f32, rotation.y as f32, rotation.z as f32, rotation.w as f32).normalize()
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
        ApplyOutcome::Introduced, ApplyOutcome::RegionAdopted, ApplyOutcome::RegionPatched, Kind::Object, ObjectObservation,
        OrderEvidence::Ordered, ParentUpdate::Root, Pose, Region, RegionInfo, UpdateSource, WorldLimits,
    };
    use base::UUID;
    use math::Vec3;

    #[test]
    fn corrupt_parent_cycle_is_reported() {
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
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);

        let mut introduce = |id, local_id, order| {
            let result = world
                .observe_object(ObjectObservation {
                    source: UpdateSource::new(region, order, Ordered(u64::from(order)), 0),
                    id: UUID::from_u128(u128::from(id)),
                    local_id: NonZeroU32::new(local_id).expect("nonzero local ID"),
                    kind: Object,
                    parent: Root,
                    pose: None,
                    linear_motion: None,
                    angular_velocity: None,
                    scale: None,
                    attachment_point: None,
                    shape: None,
                    appearance: None,
                    surface_appearance: None,
                    render_materials: None,
                })
                .unwrap();
            let Introduced(entity) = result.outcome else { panic!("expected object introduction") };
            entity
        };
        let first = introduce(1_u32, 1, 1);
        let second = introduce(2_u32, 2, 2);

        world.data_mut(first).parent = Parent::Resolved(second);
        world.data_mut(second).parent = Parent::Resolved(first);

        assert_eq!(world.object(first).unwrap().world_pose(), Err(PoseError::ParentCycle));
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
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
        assert_eq!(
            world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap().outcome,
            RegionPatched
        );
        let result = world
            .observe_object(ObjectObservation {
                source: UpdateSource::new(region, 1, Ordered(1), 0),
                id: UUID::from_u128(3),
                local_id: NonZeroU32::new(1).unwrap(),
                kind: Object,
                parent: Root,
                pose: Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) }),
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: None,
                shape: None,
                appearance: None,
                surface_appearance: None,
                render_materials: None,
            })
            .unwrap();
        let Introduced(entity) = result.outcome else { panic!("expected object introduction") };
        world.transform_mut(entity).rotation = Some(Quat::from_xyzw(0.0, 0.0, 0.0, 2.0));

        assert_eq!(world.object(entity).unwrap().world_pose(), Err(PoseError::InvalidRotation { entity }));
    }
}
