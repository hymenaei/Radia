use std::num::NonZeroU32;

use super::Effects;
use crate::UpdateAuthority::Untracked;
use crate::history::WorldChange::EntityRemoved;
use crate::update::{FieldStamp, Stamp};
use crate::{Applied, ApplyError, ApplyOutcome, DeferredReason, Entity, Kind, LocalAddress, Parent, Region, UpdateAuthority, UpdateSource};
use crate::{World, WorldEntityRemovalReason, newer_field};

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
        let live = self.entity_at(address).map(|entity| {
            let record = self.store.object_record(entity).expect("address index contains a live entity");
            (entity, record.id, record.state.binding.fence())
        });
        if live.is_some_and(|(_, id, _)| self.self_agent == Some(id)) {
            return Ok(Applied::unchanged());
        }
        let Some(stamp) = source.stamp() else {
            return Ok(self.defer(DeferredReason::AmbiguousOrder, address));
        };
        if let Some((entity, id, binding_fence)) = live {
            if stamp <= binding_fence {
                return Ok(Applied::unchanged());
            }
            self.ensure_revision_available()?;
            let mut effects = Effects::default();
            self.remove_entity(entity, RemovalCause::Killed(stamp), None, &mut effects);
            return Ok(self.finish(ApplyOutcome::Removed(id), effects));
        }
        if let Some(tombstone) = self.tombstones.get(&address) {
            if stamp <= *tombstone {
                return Ok(Applied::unchanged());
            }
        } else if self.floors.get(&source.region).is_some_and(|floor| stamp <= *floor) {
            return Ok(self.defer(DeferredReason::AmbiguousOrder, address));
        }
        if self.tombstone_would_change(address, stamp) && self.insert_tombstone(address, stamp) {
            return Ok(Applied::new(ApplyOutcome::TombstoneRecorded(address)));
        }
        Ok(Applied::unchanged())
    }

    pub(super) fn remove_entity(&mut self, entity: Entity, cause: RemovalCause, preserve_child: Option<Entity>, effects: &mut Effects) {
        let mut first = Some((entity, cause, false));
        let mut pending = Vec::new();
        while let Some((entity, cause, expanded)) = first.take().or_else(|| pending.pop()) {
            let invalidation = cause.stamp();
            let reason = cause.reason();
            let (id, state, media_bytes) = self.store.removal_data(entity.0);
            let address = state.binding.address;
            let children = if expanded { None } else { self.children.remove(&entity) };
            if expanded || children.as_ref().is_none_or(|children| children.is_empty()) {
                self.object_media_bytes = self.object_media_bytes.saturating_sub(media_bytes);
                self.detach_parent(entity);
                self.store.remove(entity);
                if let Some(stamp) = cause.tombstone_stamp() {
                    self.insert_tombstone(address, stamp);
                }
                effects.changes.push(EntityRemoved { id, entity, address, reason });
                effects.recovery_cancellations.push(address);
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
                    .object_record(child)
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
                    effects.recovery_targets.push(child_address);
                    self.relink_changed(child, child_id, effects);
                }
            }
        }
    }

    fn advance_parent_fence(&mut self, child: Entity, invalidation: Option<Stamp>, invalidation_region: Region) {
        let Some(stamp) = invalidation
            .filter(|_| self.store.object_record(child).is_some_and(|record| record.state.binding.address.region == invalidation_region))
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
