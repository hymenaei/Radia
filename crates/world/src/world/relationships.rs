//! Parent declarations, graph indexes, frame changes, and relationship recovery.

use std::num::NonZeroU32;

use base::UUID;

use super::journal::{
    WorldChange::{EntityUpdated, SubtreeInvalidated},
    WorldChangeClasses,
};
use super::{Binding, ObjectOrdering, ObjectRecord, World, WorldChange};
use crate::update::{FieldStamp, Stamp};
use crate::{ApplyError, Entity, Kind, LocalAddress, Parent, ParentUpdate, Region};

impl World {
    pub(super) fn waiting_children_will_change(
        &self,
        address: LocalAddress,
        entity: Entity,
        parent_uuid: UUID,
        parent_binding: Binding,
    ) -> bool {
        let Some(waiting) = self.pending_children.get(&address) else { return false };
        let records = self.ecs.view::<(&ObjectRecord, &ObjectOrdering)>();
        waiting.iter().any(|child| {
            let Some((record, ordering)) = records.get(child.0) else { return false };
            match record.state.parent {
                Parent::Pending { expected_uuid, .. } if expected_uuid.is_none_or(|expected| expected == parent_uuid) => {
                    parent_binding.admits_parent_link(expected_uuid, ordering.fences.parent, parent_uuid)
                        && (!self.would_cycle(*child, entity) || expected_uuid.is_none())
                }
                _ => false,
            }
        })
    }

    pub(super) fn parent_will_be_cascaded(&self, root: Entity, target: Entity, stamp: Stamp) -> bool {
        let Some(root_region) = self.record(root).map(|record| record.state.binding.address.region()) else { return false };
        let mut current = target;
        let records = self.ecs.view::<&ObjectRecord>();

        for _ in 0..=self.entities.len() {
            if current == root {
                return true;
            }

            if current.1 != self.id {
                return false;
            }
            let Some(record) = records.get(current.0) else { return false };
            if record.kind == Kind::Avatar || record.state.binding.address.region != root_region || stamp <= record.state.binding.fence {
                return false;
            }

            let Parent::Resolved(parent) = record.state.parent else { return false };
            current = parent;
        }

        false
    }

    pub(super) fn invalidate_subtree(&self, root: Entity, changes: &mut Vec<WorldChange>) {
        if self.children.get(&root).is_some_and(|children| !children.is_empty()) {
            let id = self.record(root).expect("a subtree root is live").id;
            changes.push(SubtreeInvalidated { id, entity: root });
        }
    }

    /// Resolves an accepted parent declaration; only a current occupant can replace an expectation.
    pub(super) fn resolve_parent(
        &self,
        region: Region,
        local_id: NonZeroU32,
        update: ParentUpdate,
        stamp: FieldStamp,
        child: Option<Entity>,
    ) -> Result<Parent, ApplyError> {
        match update {
            ParentUpdate::Root => Ok(Parent::Root),
            ParentUpdate::Local(parent_local_id) => {
                let address = LocalAddress::new(region, parent_local_id);
                if child.is_some_and(|entity| self.record(entity).is_some_and(|obj| obj.state.binding.address == address))
                    || local_id == parent_local_id
                {
                    return Err(ApplyError::InvalidValue { field: "parent_cycle" });
                }
                let expected_uuid = child.and_then(|entity| self.record(entity)).and_then(|object| match object.state.parent {
                    Parent::Pending { address: pending_address, expected_uuid } if pending_address == address => expected_uuid,
                    _ => None,
                });
                if let Some(parent) = self.addresses.get(&address).copied() {
                    let object = self.record(parent).expect("address index contains a live parent");
                    if object.state.binding.admits_parent_link(expected_uuid, Some(stamp), object.id) {
                        Ok(Parent::Resolved(parent))
                    } else {
                        Ok(Parent::Pending { address, expected_uuid })
                    }
                } else {
                    Ok(Parent::Pending { address, expected_uuid })
                }
            }
        }
    }

    pub(super) fn apply_parent(&mut self, child: Entity, parent: Parent, recovery_targets: &mut Vec<LocalAddress>) {
        self.set_parent(child, parent);
        if let Parent::Pending { address, .. } = parent {
            recovery_targets.push(address);
        }
    }

    pub(super) fn set_parent(&mut self, child: Entity, parent: Parent) {
        let child_uuid = {
            let record = self.record(child).expect("a parented child is live");
            if record.state.parent == parent {
                return;
            }
            record.id
        };
        self.detach_parent(child);
        self.state_mut(child).parent = parent;
        match parent {
            Parent::Resolved(parent) => {
                self.children.entry(parent).or_default().insert(child_uuid, child);
            }
            Parent::Pending { address, .. } => {
                self.pending_children.entry(address).or_default().insert(child);
            }
            Parent::Unknown | Parent::Root => {}
        }
    }

    pub(super) fn detach_parent(&mut self, child: Entity) {
        let Some((parent, child_uuid)) = self.record(child).map(|object| (object.state.parent, object.id)) else {
            return;
        };
        match parent {
            Parent::Resolved(parent) => {
                if let Some(children) = self.children.get_mut(&parent) {
                    children.remove(&child_uuid);
                    if children.is_empty() {
                        self.children.remove(&parent);
                    }
                }
            }
            Parent::Pending { address, .. } => {
                if let Some(children) = self.pending_children.get_mut(&address) {
                    children.remove(&child);
                    if children.is_empty() {
                        self.pending_children.remove(&address);
                    }
                }
            }
            Parent::Unknown | Parent::Root => {}
        }
    }

    pub(super) fn pend_source_children(
        &mut self,
        parent: Entity,
        old_address: LocalAddress,
        parent_uuid: UUID,
        changes: &mut Vec<WorldChange>,
        recovery_targets: &mut Vec<LocalAddress>,
    ) {
        let children = self.children.remove(&parent).unwrap_or_default();
        for child in children.into_values() {
            let Some((child_parent, child_address, child_uuid)) =
                self.record(child).map(|object| (object.state.parent, object.state.binding.address, object.id))
            else {
                continue;
            };
            if child_parent != Parent::Resolved(parent) || child_address.region != old_address.region {
                continue;
            }

            self.set_parent(child, Parent::Pending { address: old_address, expected_uuid: Some(parent_uuid) });
            recovery_targets.push(child_address);
            changes.push(EntityUpdated {
                id: child_uuid,
                entity: child,
                classes: WorldChangeClasses::RELATIONSHIP | WorldChangeClasses::POSE,
            });
            self.invalidate_subtree(child, changes);
        }
    }

    pub(super) fn resolve_waiting_children(
        &mut self,
        address: LocalAddress,
        parent: Entity,
        parent_uuid: UUID,
        changes: &mut Vec<WorldChange>,
        recovery_targets: &mut Vec<LocalAddress>,
        recovery_cancellations: &mut Vec<LocalAddress>,
    ) {
        let waiting = self.pending_children.get(&address).cloned().unwrap_or_default();
        let binding = self.record(parent).expect("address index contains a live parent").state.binding;
        for child in waiting {
            let (parent_state, child_address, uuid, parent_stamp) = {
                let mut read = self.ecs.query_one::<(&ObjectRecord, &ObjectOrdering)>(child.0);
                let Ok((record, ordering)) = read.get() else { continue };
                (record.state.parent, record.state.binding.address, record.id, ordering.fences.parent)
            };
            let Parent::Pending { expected_uuid, .. } = parent_state else { continue };
            if !binding.admits_parent_link(expected_uuid, parent_stamp, parent_uuid) {
                recovery_targets.push(child_address);
                continue;
            }
            if self.would_cycle(child, parent) {
                if expected_uuid.is_none() {
                    self.set_parent(child, Parent::Pending { address, expected_uuid: Some(parent_uuid) });
                    changes.push(EntityUpdated {
                        id: uuid,
                        entity: child,
                        classes: WorldChangeClasses::RELATIONSHIP | WorldChangeClasses::POSE,
                    });
                    self.invalidate_subtree(child, changes);
                }
                recovery_targets.push(child_address);
                continue;
            }
            self.detach_parent(child);
            self.state_mut(child).parent = Parent::Resolved(parent);
            self.children.entry(parent).or_default().insert(uuid, child);
            changes.push(EntityUpdated { id: uuid, entity: child, classes: WorldChangeClasses::RELATIONSHIP | WorldChangeClasses::POSE });
            self.invalidate_subtree(child, changes);
            recovery_cancellations.push(child_address);
        }
    }

    pub(super) fn would_cycle(&self, child: Entity, mut parent: Entity) -> bool {
        if self.record(child).is_some_and(|object| object.state.parent == Parent::Resolved(parent)) {
            return false;
        }
        if parent == child {
            return true;
        }
        let Some(Parent::Resolved(next)) = self.record(parent).map(|object| object.state.parent) else { return false };
        parent = next;
        let records = self.ecs.view::<&ObjectRecord>();
        for _ in 1..self.entities.len() {
            if parent == child {
                return true;
            }
            if parent.1 != self.id {
                return false;
            }
            let Some(record) = records.get(parent.0) else { return false };
            match record.state.parent {
                Parent::Resolved(next) => parent = next,
                Parent::Pending { .. } | Parent::Unknown | Parent::Root => return false,
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::WorldLimits;
    use base::UUID;
    use math::{DVec3, Quat, Vec3};

    use super::*;
    use crate::{
        ApplyOutcome::{Introduced, Removed},
        FieldUpdate,
        Kind::{Avatar, Object},
        ObjectObservation,
        OrderEvidence::Ordered,
        Parent::Pending,
        ParentUpdate::{Local, Root},
        Pose, RegionInfo, UpdateSource, WorldRevision,
    };

    fn test_limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 1,
            max_entities: 2,
            max_entities_per_region: 2,
            max_tombstones_per_region: 2,
            max_tombstones: 2,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(2),
            max_journal_entries: 4,
            max_journal_changes: 8,
            max_region_name_bytes: 32,
            max_recovery_targets: 2,
        }
    }

    #[test]
    fn generation_limit_keeps_avatar_pending() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));
        assert!(matches!(
            world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap().outcome,
            crate::RegionOutcome::Patched { .. }
        ));
        let seat = observation(&mut world, region, 1, 1, Object, Root);
        let avatar = observation(&mut world, region, 2, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        {
            let data = world.state_mut(avatar);
            data.frame_generation = u64::MAX;
            data.position_frame_generation = Some(u64::MAX);
            data.rotation_frame_generation = Some(u64::MAX);
        }
        let removed = world.remove_object(source(region, 3), NonZeroU32::new(1).unwrap()).unwrap();

        assert_eq!(removed.outcome, Removed(UUID::from_u128(1)));
        assert!(world.object(seat).is_none());
        assert_eq!(world.object_count(), 1);
        assert_eq!(
            world.object(avatar).unwrap().parent(),
            Pending { address: LocalAddress::new(region, NonZeroU32::new(1).unwrap()), expected_uuid: Some(UUID::from_u128(1)) }
        );
        assert_eq!(removed.recovery.requests, [LocalAddress::new(region, NonZeroU32::new(2).unwrap())]);
    }

    #[test]
    fn newer_child_keeps_avatar_pending() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));
        let seat = observation(&mut world, region, 1, 1, Object, Root);
        let avatar = observation(&mut world, region, 3, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        let removed = world.remove_object(source(region, 2), NonZeroU32::new(1).unwrap()).unwrap();

        assert_eq!(removed.outcome, Removed(UUID::from_u128(1)));
        assert!(world.object(seat).is_none());
        assert_eq!(
            world.object(avatar).unwrap().parent(),
            Pending { address: LocalAddress::new(region, NonZeroU32::new(1).unwrap()), expected_uuid: Some(UUID::from_u128(1)) }
        );
        assert_eq!(removed.recovery.requests, [LocalAddress::new(region, NonZeroU32::new(2).unwrap())]);
    }

    #[test]
    fn revision_exhaustion_keeps_relationships() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));
        let parent = observation(&mut world, region, 1, 1, Object, Root);
        let avatar = observation(&mut world, region, 2, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        let parent_address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());

        world.journal.revision = WorldRevision(u64::MAX);
        assert_eq!(world.remove_object(source(region, 3), parent_address.local_id()), Err(ApplyError::RevisionExhausted));

        assert_eq!(world.revision(), WorldRevision(u64::MAX));
        assert_eq!(world.object_count(), 2);
        assert_eq!(world.entity(UUID::from_u128(1)), Some(parent));
        assert_eq!(world.entity_at(parent_address), Some(parent));
        assert_eq!(world.entity(UUID::from_u128(2)), Some(avatar));
        assert_eq!(world.object(avatar).unwrap().parent(), Parent::Resolved(parent));
        assert_eq!(world.children(parent).map(|object| object.entity()).collect::<Vec<_>>(), [avatar]);
    }

    #[test]
    fn parent_retirement_removes_avatar() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));
        let parent = observation(&mut world, region, 1, 1, Object, Root);
        let child = observation(&mut world, region, 2, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        let retired = world.retire_region(region).unwrap();
        assert!(matches!(retired.outcome, crate::RegionOutcome::Retired { entities_removed: 2, .. }));
        assert!(world.object(parent).is_none());
        assert!(world.object(child).is_none());
        assert_eq!(retired.recovery_cancellations.len(), 2);
    }

    fn observation(world: &mut World, region: Region, order: u64, local_id: u32, kind: Kind, parent: ParentUpdate) -> Entity {
        let object = ObjectObservation {
            source: source(region, order),
            id: UUID::from_u128(u128::from(local_id)),
            local_id: NonZeroU32::new(local_id).unwrap(),
            kind,
            parent,
            pose: Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) }),
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Keep,
            surface_appearance: None,
            render_materials: None,
        };
        let Introduced(entity) = world.observe_object(object).unwrap().outcome else { panic!("expected object introduction") };
        entity
    }

    fn source(region: Region, order: u64) -> UpdateSource {
        UpdateSource::new(region, order as u32, Ordered(order), 0)
    }
}
