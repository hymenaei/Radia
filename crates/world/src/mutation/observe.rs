use uuid::Uuid;

use super::Effects;
use super::fields::{Draft, Fields, Origin, Settlement, validate_uuid};
use super::lifecycle::RemovalCause;
use crate::ParentUpdate::Local;
use crate::UpdateAuthority::Untracked;
use crate::history::WorldChange::{EntityAdded, EntityRebound, EntityTransferred, EntityUpdated};
use crate::mutation::facet::FacetSet;
use crate::object::relationships::ParentResolution;
use crate::{
    Applied, ApplyError, ApplyOutcome, DeferredReason, Entity, Kind, LocalAddress, ObjectObservation, Parent, ParentUpdate, Region,
    Rejections,
};
use crate::{Binding, ObjectOrdering, ObjectRecord, ObjectState, World, newer_field};

struct Observed {
    id: Uuid,
    kind: Kind,
    parent: ParentUpdate,
    address: LocalAddress,
    origin: Origin,
    fields: Fields,
    rejections: Rejections,
}

impl World {
    pub fn observe_object(&mut self, observation: ObjectObservation) -> Result<Applied, ApplyError> {
        self.observe(observation, false)
    }

    pub fn handoff_agent(&mut self, from: Region, observation: ObjectObservation) -> Result<Applied, ApplyError> {
        self.require_active_region(from)?;
        self.require_active_region(observation.source.region)?;
        if from == observation.source.region {
            return Err(ApplyError::InvalidValue { field: "handoff_region" });
        }
        let id = self.self_agent.ok_or(ApplyError::StaleRegion)?;
        if observation.id != id {
            return Err(ApplyError::InvalidValue { field: "agent_id" });
        }
        let entity = self.entity(id).ok_or(ApplyError::StaleRegion)?;
        if self.store.object_record(entity).expect("UUID index contains a live entity").state.binding.address.region != from {
            return Err(ApplyError::StaleRegion);
        }
        self.observe(observation, true)
    }

    fn observe(&mut self, observation: ObjectObservation, allow_agent_handoff: bool) -> Result<Applied, ApplyError> {
        let ObjectObservation { source, id, local_id, kind, parent, .. } = observation;
        self.require_active_region(source.region)?;
        validate_uuid(id, "object_id")?;
        let address = LocalAddress::new(source.region, local_id);
        let Some(stamp) = source.stamp() else {
            return Ok(self.defer(DeferredReason::AmbiguousOrder, address));
        };
        let existing = self.entity(id);
        let current = existing.and_then(|entity| self.store.object_record(entity)).map(|record| record.state);
        if source.authority() == Untracked && current.is_none_or(|state| state.binding.address != address) {
            let applied = self.unknown_address(address, stamp);
            return Ok(if applied.outcome == ApplyOutcome::Unchanged { Applied::new(ApplyOutcome::IgnoredStale) } else { applied });
        }
        if let Some(applied) = self.preflight_address(address, id, stamp) {
            return Ok(applied);
        }
        let mut fields = Fields::observed(observation);
        let rejections = fields.validate(current.and_then(|state| state.transform.rotation()));
        let observed = Observed { id, kind, parent, address, origin: Origin { source, stamp }, fields, rejections };
        match existing {
            Some(entity) => self.update_observed(entity, observed, allow_agent_handoff),
            None => self.introduce(observed),
        }
    }

    fn introduce(&mut self, observed: Observed) -> Result<Applied, ApplyError> {
        let Observed { id, kind, parent: parent_update, address, origin, fields, mut rejections } = observed;
        let displaced = self.entity_at(address);
        if displaced.is_none()
            && (self.store.len() >= self.limits.max_entities
                || self.region_object_count(address.region) >= self.limits.max_entities_per_region)
        {
            return Err(ApplyError::EntityBudgetExhausted);
        }
        let address_was_reused = self.address_was_reused(address);
        let parent = match self.resolve_parent(address.region, address.local_id, parent_update, origin.field(), None) {
            ParentResolution::Accepted(parent) => parent,
            ParentResolution::Cycle => return Err(ApplyError::InvalidValue { field: "parent_cycle" }),
        };
        let parent = match displaced {
            Some(displaced) => self.pend_cascaded_parent(displaced, parent, parent_update, address.region, origin),
            None => parent,
        };
        self.ensure_revision_available()?;

        let state = ObjectState::new(Binding::new(address, 0, origin.stamp, address_was_reused.then_some(origin.stamp)));
        let mut draft = Draft::new(state, ObjectOrdering::default(), FacetSet::default());
        let frame = draft.plan_frame(false, origin)?;
        let pose_accepted = draft.accept_pose(&fields, origin, frame, &mut rejections);
        let settlement =
            Settlement { frame, pose_accepted, detach_snapshot: None, parent_applied: true, parent_cycle: false, advance_binding: false };
        draft.update(fields, kind, origin, settlement, self.object_media_bytes, &mut rejections);
        let (state, facets, ordering) = draft.into_parts();

        let mut effects = Effects::default();
        if let Some(displaced) = displaced {
            self.remove_entity(displaced, RemovalCause::AddressReplaced(origin.stamp), None, &mut effects);
        }
        self.tombstones.remove(&address);
        let facets = facets.map(|changes| changes.after).unwrap_or_default();
        let entity = self.store.insert(ObjectRecord { id, kind, state }, facets, ordering);
        self.apply_parent(entity, parent, &mut effects);
        self.resolve_waiting_children(address, entity, id, &mut effects);
        effects.changes.push(EntityAdded { id, entity, address });
        Ok(self.finish(ApplyOutcome::Introduced(entity), effects).with_rejections(rejections).with_recovery_satisfied(true))
    }

    fn update_observed(&mut self, entity: Entity, observed: Observed, allow_agent_handoff: bool) -> Result<Applied, ApplyError> {
        let Observed { id, kind, parent: parent_update, address, origin, mut fields, mut rejections } = observed;
        let source = origin.source;
        let mut read = self.store.read(entity.0);
        let (record, ordering) = read.get().expect("UUID index contains a complete entity");
        let before = record.state;
        let old_binding = before.binding;
        if record.kind != kind {
            return Err(ApplyError::KindMismatch { current: record.kind, observed: kind });
        }
        let transfer = old_binding.address.region != source.region;
        let facets = if transfer || fields.has_facet_updates() {
            self.store.facets(entity.0, &fields.facets, transfer)
        } else {
            FacetSet::default()
        };
        if transfer && self.self_agent == Some(id) && !allow_agent_handoff {
            return Err(ApplyError::InvalidValue { field: "agent_handoff_required" });
        }
        let binding_changed = old_binding.address != address;
        let stale = if binding_changed {
            !transfer && origin.stamp <= old_binding.fence()
        } else {
            !fields.is_newer_than(&before, &ordering.fences, &facets, kind, origin)
        };
        if stale {
            return Ok(Applied::new(ApplyOutcome::IgnoredStale).with_rejections(rejections));
        }
        if transfer && self.entity_at(address).is_none() && self.region_object_count(source.region) >= self.limits.max_entities_per_region {
            return Err(ApplyError::EntityBudgetExhausted);
        }
        let address_was_reused = self.address_was_reused(address);
        let displaced = self.entity_at(address).filter(|displaced| *displaced != entity);

        let parent_admitted =
            (transfer || newer_field(origin.field(), ordering.fences.parent)) && ordering.admits_parent(source, origin.stamp);
        let (parent, parent_cycle) = if parent_admitted {
            match self.resolve_parent(source.region, address.local_id, parent_update, origin.field(), Some(entity)) {
                ParentResolution::Accepted(parent) => (parent, false),
                ParentResolution::Cycle => (Parent::Unknown, true),
            }
        } else {
            (before.parent, false)
        };
        if parent_cycle && binding_changed {
            return Err(ApplyError::InvalidValue { field: "parent_cycle" });
        }
        let parent_applied = parent_admitted && !parent_cycle;
        let parent = match displaced {
            Some(displaced) if parent_applied => self.pend_cascaded_parent(displaced, parent, parent_update, source.region, origin),
            _ => parent,
        };
        fields.reject_parent(parent_cycle, source.authority() == Untracked && ordering.protects_parent(), &mut rejections);
        if source.authority() == Untracked {
            fields.reject_untrusted(&ordering.fences, &facets, &mut rejections);
        }
        let address_generation = if binding_changed {
            old_binding.address_generation.checked_add(1).ok_or(ApplyError::AddressGenerationExhausted)?
        } else {
            old_binding.address_generation
        };
        let next_parent = if parent_applied { parent } else { before.parent };
        let next_attachment = fields.next_attachment_point(before.attachment_point, &ordering.fences, origin);
        let frame_changed =
            transfer || self.relationship_frame_changed(before.parent, next_parent, before.attachment_point, next_attachment);
        let detach_snapshot =
            (parent_applied && !transfer).then(|| self.detach_snapshot(entity, kind, before.parent, next_parent)).flatten();

        let mut draft = Draft::new(before, *ordering, facets);
        if transfer {
            draft.state.reset_region_relative();
            draft.ordering.reset_region_relative();
            draft.reset_region_relative();
        }
        if binding_changed {
            draft.state.binding = Binding::new(address, address_generation, origin.stamp, address_was_reused.then_some(origin.stamp));
        }
        let frame = draft.plan_frame(frame_changed, origin)?;
        let pose_accepted = draft.accept_pose(&fields, origin, frame, &mut rejections);
        let pose_affects_descendants = !transfer && pose_accepted && draft.pose_changes(fields.pose);
        let settlement = Settlement { frame, pose_accepted, detach_snapshot, parent_applied, parent_cycle, advance_binding: true };
        draft.update(fields, kind, origin, settlement, self.object_media_bytes, &mut rejections);
        let parent_changed = parent_applied && draft.state.parent != parent;
        let change_classes = draft.change_classes(&before, if parent_applied { parent } else { draft.state.parent });
        let waiting_children_change =
            (parent_applied || binding_changed) && self.waiting_children_will_change(address, entity, id, draft.state.binding);
        if binding_changed || displaced.is_some() || !change_classes.is_empty() || waiting_children_change {
            self.ensure_revision_available()?;
        }
        let (state, facets, ordering) = draft.into_parts();
        drop(read);

        let mut effects = Effects::default();
        if let Some(displaced) = displaced {
            self.remove_entity(displaced, RemovalCause::AddressReplaced(origin.stamp), Some(entity), &mut effects);
        }
        if binding_changed {
            effects.recovery_cancellations.push(old_binding.address);
            effects.changes.push(if transfer {
                EntityTransferred { id, entity, from: old_binding.address, to: address }
            } else {
                EntityRebound { id, entity, from: old_binding.address, to: address }
            });
            if transfer {
                self.pend_source_children(entity, old_binding.address, id, &mut effects);
                self.set_parent(entity, Parent::Unknown);
            }
            self.tombstones.remove(&address);
        }
        self.store.commit(entity, state, facets, ordering);
        if binding_changed {
            self.insert_tombstone(old_binding.address, if transfer { old_binding.fence() } else { origin.stamp });
        }
        if parent_changed {
            self.apply_parent(entity, parent, &mut effects);
        }
        if parent_applied || binding_changed {
            self.resolve_waiting_children(address, entity, id, &mut effects);
        }
        if frame_changed || parent_changed || pose_affects_descendants {
            self.invalidate_subtree(entity, &mut effects);
        }
        if !change_classes.is_empty() {
            effects.changes.push(EntityUpdated { id, entity, classes: change_classes });
        }
        let outcome = if effects.changes.is_empty() {
            ApplyOutcome::Unchanged
        } else if transfer && self.self_agent == Some(id) {
            ApplyOutcome::AgentHandedOff(entity)
        } else {
            ApplyOutcome::Updated(entity)
        };
        let recovery_satisfied = matches!(outcome, ApplyOutcome::Updated(_) | ApplyOutcome::Unchanged)
            && (!matches!(before.parent, Parent::Pending { .. }) || parent_applied);
        Ok(self.finish(outcome, effects).with_rejections(rejections).with_recovery_satisfied(recovery_satisfied))
    }

    fn region_object_count(&self, region: Region) -> usize {
        self.store.region_ids(region).map_or(0, |ids| ids.len())
    }

    fn address_was_reused(&self, address: LocalAddress) -> bool {
        self.entity_at(address).is_some() || self.tombstones.contains_key(&address) || self.floors.contains_key(&address.region)
    }

    fn pend_cascaded_parent(&self, displaced: Entity, parent: Parent, update: ParentUpdate, region: Region, origin: Origin) -> Parent {
        if let (Parent::Resolved(parent_entity), Local(parent_local_id)) = (parent, update)
            && self.parent_will_be_cascaded(displaced, parent_entity, origin.stamp)
        {
            let expected_uuid = self.store.object_record(parent_entity).expect("resolved parent is live before the commit").id;
            return Parent::Pending { address: LocalAddress::new(region, parent_local_id), expected_uuid: Some(expected_uuid) };
        }
        parent
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use math::Vec3;
    use uuid::Uuid;

    use crate::{
        ApplyError, ApplyError::RevisionExhausted, ApplyOutcome, Kind, LinearMotion, LocalAddress, MotionUpdateKind, ObjectObservation,
        ObjectPatch, OrderEvidence::Ordered, ParentUpdate::Root, Pose, Region, UpdateSource, WorldRevision,
    };

    use crate::mutation::tests::{observation, test_world};

    #[test]
    fn handoff_revision_exhaustion_is_atomic() {
        let source_region = Region::new(25);
        let destination = Region::new(26);
        let mut world = test_world(&[source_region, destination]);
        let agent_id = Uuid::from_u128(25);
        world.set_self_agent(agent_id).unwrap();
        let observation = ObjectObservation {
            pose: Some(Pose { position: Some(Vec3::new(4.0, 5.0, 6.0)), rotation: None }),
            linear_motion: Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) }),
            angular_velocity: Some(Vec3::Z),
            scale: Some(Vec3::splat(1.5)),
            ..ObjectObservation::new(
                UpdateSource::new(source_region, 1, Ordered(1), 0),
                agent_id,
                NonZeroU32::new(8).unwrap(),
                Kind::Avatar,
                Root,
            )
        };
        let source_local_id = observation.local_id;
        let introduced = world.observe_object(observation).unwrap();
        let ApplyOutcome::Introduced(agent) = introduced.outcome else { panic!("expected agent introduction") };
        let source_address = LocalAddress::new(source_region, source_local_id);
        let destination_address = LocalAddress::new(destination, NonZeroU32::new(9).unwrap());
        world.journal.revision = WorldRevision(u64::MAX);

        let result = world.handoff_agent(
            source_region,
            ObjectObservation::new(
                UpdateSource::new(destination, 2, Ordered(2), 0),
                agent_id,
                destination_address.local_id(),
                Kind::Avatar,
                Root,
            ),
        );

        assert_eq!(result, Err(RevisionExhausted));
        assert_eq!(world.revision(), WorldRevision(u64::MAX));
        assert_eq!(world.entity(agent_id), Some(agent));
        assert_eq!(world.entity_at(source_address), Some(agent));
        assert_eq!(world.entity_at(destination_address), None);
        assert_eq!(world.objects_in_region(source_region).count(), 1);
        assert_eq!(world.objects_in_region(destination).count(), 0);
        let current = world.object(agent).unwrap();
        assert_eq!(current.address(), source_address);
        assert_eq!(current.position(), Some(Vec3::new(4.0, 5.0, 6.0)));
        assert_eq!(current.scale(), Some(Vec3::splat(1.5)));
        assert_eq!(current.linear_motion(), LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
        assert_eq!(current.angular_velocity(), Some(Vec3::Z));
    }

    #[test]
    fn full_and_terse_motion_bounds() {
        let region = Region::new(28);
        let mut world = test_world(&[region]);
        let mut full = observation(region, 28, 1, 1);
        full.linear_motion = Some(LinearMotion { velocity: Some(Vec3::splat(1024.0)), acceleration: Some(Vec3::splat(-1024.0)) });
        full.angular_velocity = Some(Vec3::splat(1024.0));
        let ApplyOutcome::Introduced(entity) = world.observe_object(full).unwrap().outcome else { panic!("expected object introduction") };

        let accepted = world.object(entity).unwrap();
        assert_eq!(accepted.linear_motion().velocity, Some(Vec3::splat(1024.0)));
        assert_eq!(accepted.linear_motion().acceleration, Some(Vec3::splat(-1024.0)));
        assert_eq!(accepted.angular_velocity(), Some(Vec3::splat(1024.0)));

        let mut absurd = observation(region, 28, 1, 2);
        absurd.linear_motion = Some(LinearMotion { velocity: Some(Vec3::splat(f32::MAX)), acceleration: None });
        absurd.angular_velocity = Some(Vec3::splat(f32::MAX));
        let rejected = world.observe_object(absurd).unwrap();
        assert!(!rejected.rejections.is_empty());
        let accepted = world.object(entity).unwrap();
        assert_eq!(accepted.linear_motion().velocity, Some(Vec3::splat(1024.0)));
        assert_eq!(accepted.angular_velocity(), Some(Vec3::splat(1024.0)));

        let mut terse = ObjectPatch::new(
            UpdateSource::new(region, 3, Ordered(3), 0).with_motion_update_kind(MotionUpdateKind::Terse),
            NonZeroU32::new(1).unwrap(),
        );
        terse.linear_motion = LinearMotion { velocity: Some(Vec3::splat(128.0)), acceleration: Some(Vec3::splat(64.0)) };
        terse.angular_velocity = Some(Vec3::splat(64.0));
        let _ = world.patch_object(terse).unwrap();
        let accepted = world.object(entity).unwrap();
        assert_eq!(accepted.linear_motion().velocity, Some(Vec3::splat(128.0)));
        assert_eq!(accepted.linear_motion().acceleration, Some(Vec3::splat(64.0)));
        assert_eq!(accepted.angular_velocity(), Some(Vec3::splat(64.0)));
    }

    #[test]
    fn compressed_source_tracks_missing_omega() {
        let region = Region::new(29);
        let mut world = test_world(&[region]);
        let mut initial = observation(region, 29, 1, 1);
        initial.angular_velocity = Some(Vec3::ZERO);
        initial.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: None });
        let ApplyOutcome::Introduced(entity) = world.observe_object(initial).unwrap().outcome else {
            panic!("expected object introduction")
        };
        assert!(world.object(entity).unwrap().motion_source().unwrap().has_explicit_angular_velocity());

        let mut compressed = observation(region, 29, 1, 2);
        compressed.linear_motion = Some(LinearMotion { velocity: None, acceleration: Some(Vec3::ZERO) });
        let _ = world.observe_object(compressed).unwrap();

        let object = world.object(entity).unwrap();
        assert_eq!(object.angular_velocity(), Some(Vec3::ZERO));
        assert!(!object.motion_source().unwrap().has_explicit_angular_velocity());
    }

    #[test]
    fn generation_overflow_preserves_bindings() {
        let region = Region::new(27);
        let mut world = test_world(&[region]);
        let ApplyOutcome::Introduced(first) = world.observe_object(observation(region, 27, 1, 1)).unwrap().outcome else {
            panic!("expected first object introduction")
        };
        let ApplyOutcome::Introduced(displaced) = world.observe_object(observation(region, 28, 2, 2)).unwrap().outcome else {
            panic!("expected second object introduction")
        };
        let old_address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        let occupied_address = LocalAddress::new(region, NonZeroU32::new(2).unwrap());
        world.state_mut(first).binding.address_generation = u64::MAX;

        let revision = world.revision();
        let journal = world.journal.entries.iter().map(|entry| (entry.revision, entry.changes.clone())).collect::<Vec<_>>();
        let result = world.observe_object(observation(region, 27, 2, 3));

        assert!(matches!(result, Err(ApplyError::AddressGenerationExhausted)));
        assert_eq!(world.revision(), revision);
        assert_eq!(world.journal.entries.iter().map(|entry| (entry.revision, entry.changes.clone())).collect::<Vec<_>>(), journal);
        assert_eq!(world.entity(Uuid::from_u128(27)), Some(first));
        assert_eq!(world.entity(Uuid::from_u128(28)), Some(displaced));
        assert_eq!(world.entity_at(old_address), Some(first));
        assert_eq!(world.entity_at(occupied_address), Some(displaced));
        assert_eq!(world.object_count(), 2);
    }
}
