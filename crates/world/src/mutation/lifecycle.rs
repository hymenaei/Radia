use std::{num::NonZeroU32, time::Instant};

use crate::UpdateAuthority::Untracked;
use crate::history::{
    WorldChange::{EntityRemoved, EntityUpdated},
    WorldChangeClasses,
};
use crate::update::{FieldStamp, Stamp};
use crate::{
    Applied, ApplyError, ApplyOutcome, DeferredReason, Entity, Kind, LocalAddress, ObjectMedia, Parent, PoseState, Recovery, Region,
    UpdateAuthority, UpdateSource,
};
use crate::{RecentBatch, RecentObject, World, WorldChange, WorldEntityRemovalReason, newer_field};

#[derive(Clone, Copy)]
pub(super) enum RemovalCause {
    Killed(Stamp),
    AddressReplaced(Stamp),
    ParentCascade(Stamp),
    RegionRetired,
}

impl RemovalCause {
    fn stamp(self) -> Option<Stamp> {
        match self {
            Self::Killed(stamp) | Self::AddressReplaced(stamp) | Self::ParentCascade(stamp) => Some(stamp),
            Self::RegionRetired => None,
        }
    }
    fn reason(self) -> WorldEntityRemovalReason {
        match self {
            Self::Killed(_) => WorldEntityRemovalReason::Killed,
            Self::AddressReplaced(_) => WorldEntityRemovalReason::AddressReplaced,
            Self::ParentCascade(_) => WorldEntityRemovalReason::ParentCascade,
            Self::RegionRetired => WorldEntityRemovalReason::RegionRetired,
        }
    }
    fn tombstone_stamp(self) -> Option<Stamp> {
        match self {
            Self::Killed(stamp) | Self::ParentCascade(stamp) => Some(stamp),
            Self::AddressReplaced(_) | Self::RegionRetired => None,
        }
    }
}

impl World {
    pub fn remove_object(&mut self, source: UpdateSource, local_id: NonZeroU32) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        if source.authority() == Untracked {
            return Ok(Applied::unchanged());
        }
        let address = LocalAddress::new(source.region, local_id);
        if self.entity_at(address).is_some_and(|entity| self.store.record(entity).is_some_and(|record| self.self_agent == Some(record.id)))
        {
            return Ok(Applied::unchanged());
        }
        let Some(stamp) = source.stamp() else {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![address], self.limits.max_recovery_targets),
            ));
        };
        if let Some(entity) = self.entity_at(address) {
            let (id, binding_fence) = {
                let record = self.store.record(entity).expect("address index contains a live entity");
                (record.id, record.state.binding.fence())
            };
            if stamp <= binding_fence {
                return Ok(Applied::unchanged());
            }
            self.ensure_revision_available()?;
            let mut changes = Vec::new();
            let mut recovery_targets = Vec::new();
            let mut recovery_cancellations = Vec::new();
            let mut recent = RecentBatch::default();
            self.remove_entity(
                entity,
                RemovalCause::Killed(stamp),
                None,
                &mut changes,
                &mut recovery_targets,
                &mut recovery_cancellations,
                &mut recent,
            );
            self.recent.apply_batch(recent);
            return Ok(self
                .finish_mutation(
                    ApplyOutcome::Removed(id),
                    changes,
                    Recovery::from_targets(recovery_targets, self.limits.max_recovery_targets),
                )
                .with_recovery_cancellations(recovery_cancellations));
        }
        if let Some(tombstone) = self.tombstones.get(&address) {
            if stamp <= *tombstone {
                return Ok(Applied::unchanged());
            }
        } else if self.floors.get(&source.region).is_some_and(|floor| stamp <= *floor) {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![address], self.limits.max_recovery_targets),
            ));
        }
        if !self.tombstone_would_change(address, stamp) {
            return Ok(Applied::unchanged());
        }
        if self.insert_tombstone(address, stamp) {
            return Ok(self.finish_mutation(ApplyOutcome::TombstoneRecorded(address), Vec::new(), Recovery::default()));
        }
        Ok(Applied::unchanged())
    }
    pub(super) fn remove_entity(
        &mut self,
        entity: Entity,
        cause: RemovalCause,
        preserve_child: Option<Entity>,
        changes: &mut Vec<WorldChange>,
        recovery_targets: &mut Vec<LocalAddress>,
        recovery_cancellations: &mut Vec<LocalAddress>,
        recent_batch: &mut RecentBatch,
    ) {
        let mut first = Some((entity, cause, false));
        let mut pending = Vec::new();
        while let Some((entity, cause, expanded)) = first.take().or_else(|| pending.pop()) {
            let invalidation = cause.stamp();
            let reason = cause.reason();
            let (id, address, media_bytes, recent) = self
                .object(entity)
                .map(|object| {
                    let recent = (!expanded).then(|| RecentObject {
                        id: object.record.id,
                        kind: object.kind(),
                        address: object.address(),
                        attachment_point: object.attachment_point(),
                        properties: object.properties(),
                        properties_provenance: object.properties_provenance(),
                        last_pose: match object.world_pose() {
                            Ok(PoseState::Resolved(pose)) => Some(pose),
                            Ok(PoseState::Unresolved(_)) | Ok(PoseState::Provisional { .. }) | Err(_) => None,
                        },
                        removal_reason: reason,
                    });
                    (
                        object.record.id,
                        object.address(),
                        object.content().object_media.as_deref().map_or(0, ObjectMedia::retained_bytes),
                        recent,
                    )
                })
                .expect("removal traversal contains only live entities");
            if let Some(recent) = recent {
                recent_batch.push(recent, Instant::now());
            }
            let children = if expanded { None } else { self.children.remove(&entity) };
            if expanded || children.as_ref().is_none_or(|children| children.is_empty()) {
                self.object_media_bytes = self.object_media_bytes.saturating_sub(media_bytes);
                self.detach_parent(entity);
                self.store.remove(entity);
                if let Some(stamp) = cause.tombstone_stamp() {
                    self.insert_tombstone(address, stamp);
                }
                changes.push(EntityRemoved { id, entity, address, reason });
                recovery_cancellations.push(address);
                continue;
            }
            let children = children.expect("an unexpanded parent has children");
            pending.push((entity, cause, true));
            for child in children.into_values() {
                if Some(child) == preserve_child {
                    self.set_parent(child, Parent::Unknown);
                    continue;
                }
                let Some((child_kind, child_id, child_address, child_fence)) = self
                    .store
                    .record(child)
                    .map(|record| (record.kind, record.id, record.state.binding.address, record.state.binding.fence()))
                else {
                    continue;
                };
                let cascade_stamp = invalidation
                    .filter(|stamp| address.region == child_address.region && *stamp > child_fence && child_kind != Kind::Avatar);
                if reason == WorldEntityRemovalReason::RegionRetired && child_address.region == address.region {
                    pending.push((child, RemovalCause::RegionRetired, false));
                } else if let Some(stamp) = cascade_stamp {
                    pending.push((child, RemovalCause::ParentCascade(stamp), false));
                } else {
                    self.set_parent(child, Parent::Pending { address, expected_uuid: Some(id) });
                    self.advance_parent_fence(child, invalidation, address.region);
                    recovery_targets.push(child_address);
                    changes.push(EntityUpdated {
                        id: child_id,
                        entity: child,
                        classes: WorldChangeClasses::RELATIONSHIP | WorldChangeClasses::POSE,
                    });
                    self.invalidate_subtree(child, changes);
                }
            }
        }
    }
    fn advance_parent_fence(&mut self, child: Entity, invalidation: Option<Stamp>, invalidation_region: Region) {
        let Some(stamp) = invalidation
            .filter(|_| self.store.record(child).is_some_and(|record| record.state.binding.address.region == invalidation_region))
        else {
            return;
        };
        let ordering = self.store.ordering_mut(child.0);
        let field_stamp = FieldStamp { authority: UpdateAuthority::Tracked, stamp };
        if newer_field(field_stamp, ordering.fences.parent) {
            ordering.fences.parent = Some(field_stamp);
        }
    }
}
