use std::{
    collections::{BTreeSet, HashMap},
    num::NonZeroU32,
};

use base::UUID;

use super::journal::{
    Change::{EntityRemoved, EntityUpdated, SubtreeInvalidated},
    ChangeClasses,
};
use super::{
    ApplyError, Change, Entity, EntityRemovalReason, FieldStamp, Kind, LocalAddress, ObjectMedia, Parent, ParentUpdate, Region, Stamp,
    UpdateAuthority, World, WorldLimits, newer_field, remove_index,
};

#[derive(Debug, Default)]
pub(super) struct Tombstones {
    by_address: HashMap<LocalAddress, Stamp>,
    oldest_by_region: HashMap<Region, BTreeSet<(Stamp, NonZeroU32)>>,
}

impl Tombstones {
    pub(super) fn get(&self, address: &LocalAddress) -> Option<&Stamp> {
        self.by_address.get(address)
    }

    pub(super) fn contains_key(&self, address: &LocalAddress) -> bool {
        self.by_address.contains_key(address)
    }

    pub(super) fn len(&self) -> usize {
        self.by_address.len()
    }

    pub(super) fn insert(&mut self, address: LocalAddress, stamp: Stamp) -> Option<Stamp> {
        let previous = self.remove(&address);
        self.by_address.insert(address, stamp);
        let inserted = self.oldest_by_region.entry(address.region()).or_default().insert((stamp, address.local_id()));
        assert!(inserted, "tombstone address index and eviction index disagree");
        previous
    }

    pub(super) fn remove(&mut self, address: &LocalAddress) -> Option<Stamp> {
        let stamp = self.by_address.remove(address)?;
        let region = address.region();
        let entries = self.oldest_by_region.get_mut(&region).expect("every tombstone has a region index");
        assert!(entries.remove(&(stamp, address.local_id())), "tombstone eviction index is missing an address");
        if entries.is_empty() {
            self.oldest_by_region.remove(&region);
        }
        Some(stamp)
    }

    pub(super) fn remove_region(&mut self, region: Region) {
        if let Some(entries) = self.oldest_by_region.remove(&region) {
            for (_, local_id) in entries {
                self.by_address.remove(&LocalAddress::new(region, local_id));
            }
        }
    }

    fn region_len(&self, region: Region) -> usize {
        self.oldest_by_region.get(&region).map_or(0, BTreeSet::len)
    }

    fn oldest_in_region(&self, region: Region) -> Option<(Stamp, NonZeroU32)> {
        self.oldest_by_region.get(&region)?.first().copied()
    }
}

impl World {
    pub(super) fn invalidate_subtree(&self, root: Entity, changes: &mut Vec<Change>) {
        if self.children.get(&root).is_some_and(|children| !children.is_empty()) {
            let id = self.object(root).expect("a subtree root is live").identity.id;
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
                if child.is_some_and(|entity| self.object(entity).is_some_and(|obj| obj.address() == address))
                    || local_id == parent_local_id
                {
                    return Err(ApplyError::InvalidValue { field: "parent_cycle" });
                }
                let expected_uuid = child.and_then(|entity| self.object(entity)).and_then(|object| match object.data.parent {
                    Parent::Pending { address: pending_address, expected_uuid } if pending_address == address => expected_uuid,
                    _ => None,
                });
                if let Some(parent) = self.addresses.get(&address).copied() {
                    let object = self.object(parent).expect("address index contains a live parent");
                    if object.data.binding.admits_parent_link(expected_uuid, Some(stamp), object.identity.id) {
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

    fn set_parent(&mut self, child: Entity, parent: Parent) {
        self.detach_parent(child);
        self.data_mut(child).parent = parent;
        match parent {
            Parent::Resolved(parent) => {
                self.children.entry(parent).or_default().insert(child);
            }
            Parent::Pending { address, .. } => {
                self.pending_children.entry(address).or_default().insert(child);
            }
            Parent::Unknown | Parent::Root => {}
        }
    }

    pub(super) fn detach_parent(&mut self, child: Entity) {
        let Some(parent) = self.object(child).map(|object| object.data.parent) else { return };
        match parent {
            Parent::Resolved(parent) => remove_index(&mut self.children, parent, child),
            Parent::Pending { address, .. } => remove_index(&mut self.pending_children, address, child),
            Parent::Unknown | Parent::Root => {}
        }
    }

    pub(super) fn pend_source_children(
        &mut self,
        parent: Entity,
        old_address: LocalAddress,
        parent_uuid: UUID,
        changes: &mut Vec<Change>,
        recovery_targets: &mut Vec<LocalAddress>,
    ) {
        let children = self.children.get(&parent).cloned().unwrap_or_default();
        for child in children {
            let Some((child_parent, child_address, child_uuid)) =
                self.object(child).map(|object| (object.data.parent, object.address(), object.identity.id))
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
                classes: ChangeClasses::RELATIONSHIP | ChangeClasses::TRANSFORM_MOTION,
            });
            self.invalidate_subtree(child, changes);
        }
    }

    pub(super) fn resolve_waiting_children(
        &mut self,
        address: LocalAddress,
        parent: Entity,
        parent_uuid: UUID,
        changes: &mut Vec<Change>,
        recovery_targets: &mut Vec<LocalAddress>,
        recovery_cancellations: &mut Vec<LocalAddress>,
    ) {
        let waiting = self.pending_children.get(&address).cloned().unwrap_or_default();
        let binding = self.object(parent).expect("address index contains a live parent").data.binding;
        for child in waiting {
            let Some((parent_state, child_address, uuid, parent_stamp)) =
                self.object(child).map(|object| (object.data.parent, object.address(), object.identity.id, object.data.fences.parent))
            else {
                continue;
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
                        classes: ChangeClasses::RELATIONSHIP | ChangeClasses::TRANSFORM_MOTION,
                    });
                    self.invalidate_subtree(child, changes);
                }
                recovery_targets.push(child_address);
                continue;
            }
            self.detach_parent(child);
            self.data_mut(child).parent = Parent::Resolved(parent);
            self.children.entry(parent).or_default().insert(child);
            changes.push(EntityUpdated { id: uuid, entity: child, classes: ChangeClasses::RELATIONSHIP | ChangeClasses::TRANSFORM_MOTION });
            self.invalidate_subtree(child, changes);
            recovery_cancellations.push(child_address);
        }
    }

    pub(super) fn would_cycle(&self, child: Entity, mut parent: Entity) -> bool {
        for _ in 0..self.entities.len() {
            if parent == child {
                return true;
            }
            let Some(object) = self.object(parent) else { return false };
            match object.data.parent {
                Parent::Resolved(next) => parent = next,
                Parent::Pending { .. } | Parent::Unknown | Parent::Root => return false,
            }
        }
        true
    }

    pub(super) fn remove_entity(
        &mut self,
        entity: Entity,
        invalidation: Option<Stamp>,
        tombstone: bool,
        reason: EntityRemovalReason,
        preserve_child: Option<Entity>,
        changes: &mut Vec<Change>,
        recovery_targets: &mut Vec<LocalAddress>,
        recovery_cancellations: &mut Vec<LocalAddress>,
    ) {
        if let Some(child) = preserve_child
            && self.object(child).is_some()
        {
            self.detach_parent(child);
            self.data_mut(child).parent = Parent::Unknown;
        }

        let mut pending = vec![(entity, invalidation, tombstone, reason, false)];
        while let Some((entity, invalidation, tombstone, reason, expanded)) = pending.pop() {
            let Some((id, address, binding_fence, media_bytes)) = self.object(entity).map(|object| {
                (
                    object.identity.id,
                    object.address(),
                    object.data.binding.fence,
                    object.data.object_media.as_deref().map_or(0, ObjectMedia::retained_bytes),
                )
            }) else {
                continue;
            };

            if expanded {
                self.object_media_bytes = self.object_media_bytes.saturating_sub(media_bytes);
                self.detach_parent(entity);
                self.children.remove(&entity);
                self.entities.remove(&id);
                assert!(self.object_ids.remove(&id), "UUID ordering index is missing a live entity ID");
                remove_index(&mut self.region_entities, address.region, entity);
                if self.addresses.get(&address) == Some(&entity) {
                    self.addresses.remove(&address);
                    if tombstone {
                        self.insert_tombstone(address, invalidation.unwrap_or(binding_fence));
                    }
                }
                let _ = self.ecs.despawn(entity.0);
                changes.push(EntityRemoved { id, entity, address, reason });
                recovery_cancellations.push(address);
                continue;
            }

            let children = self.children.get(&entity).cloned().unwrap_or_default();
            pending.push((entity, invalidation, tombstone, reason, true));
            for child in children {
                if Some(child) == preserve_child {
                    continue;
                }
                let Some((child_kind, child_id, child_address, child_fence)) =
                    self.object(child).map(|object| (object.kind(), object.identity.id, object.address(), object.data.binding.fence))
                else {
                    continue;
                };
                let can_retire = invalidation.is_some_and(|stamp| address.region == child_address.region && stamp > child_fence);

                if reason == EntityRemovalReason::RegionRetired && child_address.region == address.region {
                    pending.push((child, None, false, EntityRemovalReason::RegionRetired, false));
                } else if can_retire && child_kind != Kind::Avatar {
                    pending.push((child, invalidation, true, EntityRemovalReason::ParentCascade, false));
                } else {
                    self.set_parent(child, Parent::Pending { address, expected_uuid: Some(id) });
                    self.advance_parent_fence(child, invalidation, address.region);
                    recovery_targets.push(child_address);
                    changes.push(EntityUpdated {
                        id: child_id,
                        entity: child,
                        classes: ChangeClasses::RELATIONSHIP | ChangeClasses::TRANSFORM_MOTION,
                    });
                    self.invalidate_subtree(child, changes);
                }
            }
        }
    }

    pub(super) fn tombstone_would_change(&self, address: LocalAddress, stamp: Stamp) -> bool {
        if self.addresses.contains_key(&address) || self.tombstones.get(&address).is_some_and(|existing| *existing >= stamp) {
            return false;
        }
        if self.max_tombstones() == 0 || self.max_tombstones_per_region() == 0 {
            return self.floors.get(&address.region).is_none_or(|floor| stamp > *floor);
        }
        true
    }

    pub(super) fn insert_tombstone(&mut self, address: LocalAddress, stamp: Stamp) -> bool {
        if self.addresses.contains_key(&address) || self.tombstones.get(&address).is_some_and(|existing| *existing >= stamp) {
            return false;
        }
        if self.max_tombstones() == 0 || self.max_tombstones_per_region() == 0 {
            let replaced = self.tombstones.remove(&address).is_some();
            return self.advance_floor(address.region, stamp) || replaced;
        }
        let replaced = self.tombstones.insert(address, stamp).is_some();
        let mut changed = true;
        loop {
            if self.tombstones.region_len(address.region()) <= self.max_tombstones_per_region()
                && self.tombstones.len() <= self.max_tombstones()
            {
                break;
            }
            let Some((evicted_stamp, local_id)) = self.tombstones.oldest_in_region(address.region()) else { break };
            let key = LocalAddress::new(address.region(), local_id);
            self.tombstones.remove(&key);
            let floor_changed = self.advance_floor(key.region, evicted_stamp);
            if key == address {
                changed = replaced || floor_changed;
            }
        }
        changed
    }

    fn max_tombstones_per_region(&self) -> usize {
        let byte_limit = self.limits.max_tombstone_bytes_per_region / WorldLimits::tombstone_bytes_for_count(1);
        self.limits.max_tombstones_per_region.min(byte_limit)
    }

    fn max_tombstones(&self) -> usize {
        let byte_limit = self.limits.max_tombstone_bytes / WorldLimits::tombstone_bytes_for_count(1);
        self.limits.max_tombstones.min(byte_limit)
    }

    fn advance_floor(&mut self, region: Region, stamp: Stamp) -> bool {
        match self.floors.get_mut(&region) {
            Some(floor) if stamp > *floor => {
                *floor = stamp;
                true
            }
            Some(_) => false,
            None => {
                self.floors.insert(region, stamp);
                true
            }
        }
    }

    fn advance_parent_fence(&mut self, child: Entity, invalidation: Option<Stamp>, invalidation_region: Region) {
        let Some(stamp) = invalidation.filter(|_| self.object(child).is_some_and(|object| object.address().region == invalidation_region))
        else {
            return;
        };
        let mut data = self.data_mut(child);
        let field_stamp = FieldStamp { authority: UpdateAuthority::Tracked, stamp };
        if newer_field(field_stamp, data.fences.parent) {
            data.fences.parent = Some(field_stamp);
        }
    }
}

#[cfg(test)]
mod tests {
    use base::UUID;
    use math::{DVec3, Quat, Vec3};

    use super::EntityRemovalReason::RegionRetired;
    use super::*;
    use crate::{
        ApplyOutcome::{Introduced, RegionAdopted, RegionPatched, Removed},
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
    fn tombstone_index_tracks_mutations() {
        let first_region = Region::new(1);
        let second_region = Region::new(2);
        let first = LocalAddress::new(first_region, NonZeroU32::new(1).unwrap());
        let second = LocalAddress::new(first_region, NonZeroU32::new(2).unwrap());
        let other = LocalAddress::new(second_region, NonZeroU32::new(1).unwrap());
        let old = Stamp { order: 1, block_ordinal: 0 };
        let new = Stamp { order: 3, block_ordinal: 0 };
        let replacement = Stamp { order: 4, block_ordinal: 0 };
        let mut tombstones = Tombstones::default();

        assert_eq!(tombstones.insert(first, old), None);
        assert_eq!(tombstones.insert(second, new), None);
        assert_eq!(tombstones.insert(other, old), None);
        assert_eq!(tombstones.region_len(first_region), 2);
        assert_eq!(tombstones.oldest_in_region(first_region), Some((old, first.local_id())));

        assert_eq!(tombstones.insert(first, replacement), Some(old));
        assert_eq!(tombstones.oldest_in_region(first_region), Some((new, second.local_id())));
        assert_eq!(tombstones.remove(&second), Some(new));
        assert_eq!(tombstones.region_len(first_region), 1);

        tombstones.remove_region(first_region);
        assert_eq!(tombstones.len(), 1);
        assert!(tombstones.contains_key(&other));
        assert_eq!(tombstones.oldest_in_region(first_region), None);
    }

    #[test]
    fn generation_limit_keeps_avatar_pending() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
        assert_eq!(
            world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap().outcome,
            RegionPatched
        );
        let seat = observation(&mut world, region, 1, 1, Object, Root);
        let avatar = observation(&mut world, region, 2, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        {
            let mut data = world.data_mut(avatar);
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
        assert_eq!(removed.recovery.unwrap().targets, [LocalAddress::new(region, NonZeroU32::new(2).unwrap())]);
    }

    #[test]
    fn newer_child_keeps_avatar_pending() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
        let seat = observation(&mut world, region, 1, 1, Object, Root);
        let avatar = observation(&mut world, region, 3, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        let removed = world.remove_object(source(region, 2), NonZeroU32::new(1).unwrap()).unwrap();

        assert_eq!(removed.outcome, Removed(UUID::from_u128(1)));
        assert!(world.object(seat).is_none());
        assert_eq!(
            world.object(avatar).unwrap().parent(),
            Pending { address: LocalAddress::new(region, NonZeroU32::new(1).unwrap()), expected_uuid: Some(UUID::from_u128(1)) }
        );
        assert_eq!(removed.recovery.unwrap().targets, [LocalAddress::new(region, NonZeroU32::new(2).unwrap())]);
    }

    #[test]
    fn revision_exhaustion_keeps_relationships() {
        let region = Region::new(1);
        let mut world = World::new(test_limits());
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
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
        assert_eq!(world.adopt_region(region).unwrap().outcome, RegionAdopted);
        let parent = observation(&mut world, region, 1, 1, Object, Root);
        let child = observation(&mut world, region, 2, 2, Avatar, Local(NonZeroU32::new(1).unwrap()));
        let mut changes = Vec::new();
        let mut recovery_targets = Vec::new();
        let mut recovery_cancellations = Vec::new();

        world.remove_entity(parent, None, false, RegionRetired, None, &mut changes, &mut recovery_targets, &mut recovery_cancellations);

        assert!(world.object(parent).is_none());
        assert!(world.object(child).is_none());
        assert!(recovery_targets.is_empty());
        assert_eq!(recovery_cancellations.len(), 2);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().all(|change| matches!(change, EntityRemoved { reason: RegionRetired, .. })));
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
            attachment_point: None,
            shape: None,
            appearance: None,
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
