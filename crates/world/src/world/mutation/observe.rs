//! Object observations and explicit agent handoff.

use super::lifecycle::RemovalCause;
use crate::ParentUpdate::Local;

use super::super::journal::WorldChange::{EntityAdded, EntityRebound, EntityTransferred, EntityUpdated};
use super::super::journal::WorldChangeClasses;
use super::super::{
    Binding, ObjectContent, ObjectOrdering, ObjectRecord, ObjectState, ParentRejectionBarrier, RecentBatch, World, newer_field,
    remove_region_index,
};
use crate::update::FieldStamp;
use crate::{
    AgentHandoff, Applied, ApplyError, ApplyOutcome, DeferredReason, Entity, FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate,
    Kind, LinearMotion, LocalAddress, ObjectObservation, Parent, PoseState, Recovery, Region, Transform, UpdateAuthority,
};

use FieldRejectionReason::{ParentCycle, ParentDependsOnRejectedParent, UntrustedSource};
use UpdateAuthority::{Tracked, Untracked};

use super::fields::{
    apply_field_update, apply_observation_values, content_change_classes, field_at_or_after, field_is_protected, has_pose,
    mark_pose_components, observable_change_classes, parent_frame_identity_changed, pose_is_accepted, pose_update_changes_observable,
    reject_untrusted_observation_groups, retain_stored_orientation, validate_observation, validate_uuid,
};

struct ObservationPlan {
    data: ObjectState,
    content: ObjectContent,
    ordering: ObjectOrdering,
    parent_changed: bool,
    change_classes: WorldChangeClasses,
}

impl World {
    /// Accepts identity-bearing simulator evidence, introducing or updating an object.
    ///
    /// Ordering and binding evidence determine whether the observation can introduce an identity,
    /// replace an address occupant, or update an existing object. Invalid independent field groups
    /// are reported in [`Applied::rejections`] while valid groups remain eligible.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] when the operation's context, identity, or required capacity is
    /// invalid. The World remains unchanged on error.
    pub fn observe_object(&mut self, observation: ObjectObservation) -> Result<Applied, ApplyError> {
        self.observe_object_with_handoff(observation, false)
    }

    fn observe_object_with_handoff(&mut self, observation: ObjectObservation, allow_agent_handoff: bool) -> Result<Applied, ApplyError> {
        let source = observation.source;
        self.require_active_region(source.region)?;
        validate_uuid(observation.id, "object_id")?;
        let Some(stamp) = source.stamp() else {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![LocalAddress::new(source.region, observation.local_id)], self.limits.max_recovery_targets),
            ));
        };
        let address = LocalAddress::new(source.region, observation.local_id);
        let existing = self.entities.get(&observation.id).copied();
        if source.authority() == Untracked
            && !existing.is_some_and(|entity| self.record(entity).is_some_and(|record| record.state.binding.address == address))
        {
            let mut applied = self.unknown_address(address, stamp);
            if applied.outcome == ApplyOutcome::Unchanged {
                applied.outcome = ApplyOutcome::IgnoredStale;
            }
            return Ok(applied);
        }
        if let Some(t) = self.preflight_address(address, observation.id, stamp) {
            return Ok(t);
        }
        let incoming_rotation = observation.pose.and_then(|pose| pose.rotation);
        let current_rotation = existing.and_then(|entity| self.record(entity)).and_then(|record| record.state.transform.rotation());
        let (mut observation, mut rejections) = validate_observation(observation);
        if let Some(pose) = observation.pose.as_mut() {
            retain_stored_orientation(current_rotation, incoming_rotation, &mut pose.rotation);
        }
        if let Some(entity) = existing {
            let (current_kind, current_binding, binding_fence, accepts_observation) = {
                let mut read = self.ecs.query_one::<(&ObjectRecord, &ObjectOrdering)>(entity.0);
                let (record, ordering) = read.get().expect("UUID index contains a complete entity");
                (
                    record.kind,
                    record.state.binding.address,
                    record.state.binding.fence,
                    record.state.accepts_observation(ordering, &observation, stamp),
                )
            };
            if current_kind != observation.kind {
                return Err(ApplyError::KindMismatch { current: current_kind, observed: observation.kind });
            }
            if self.self_agent == Some(observation.id) && current_binding.region != source.region && !allow_agent_handoff {
                return Err(ApplyError::InvalidValue { field: "agent_handoff_required" });
            }
            if current_binding.region == source.region && current_binding != address && stamp <= binding_fence {
                return Ok(self.finish_mutation(ApplyOutcome::IgnoredStale, Vec::new(), Recovery::default()).with_rejections(rejections));
            }
            if current_binding == address && !accepts_observation {
                return Ok(self.finish_mutation(ApplyOutcome::IgnoredStale, Vec::new(), Recovery::default()).with_rejections(rejections));
            }
        } else if self.entities.len() >= self.limits.max_entities && !self.addresses.contains_key(&address) {
            return Err(ApplyError::EntityBudgetExhausted);
        }
        let address_was_reused =
            self.addresses.contains_key(&address) || self.tombstones.contains_key(&address) || self.floors.contains_key(&source.region);
        let mut current_read = existing.map(|entity| self.ecs.query_one::<(&ObjectRecord, &ObjectContent, &ObjectOrdering)>(entity.0));
        let current = current_read.as_mut().map(|read| read.get().expect("existing entity has all components"));
        let already_in_region = current.is_some_and(|(record, _, _)| record.state.binding.address.region == source.region);
        if !already_in_region
            && !self.addresses.contains_key(&address)
            && self.region_entities.get(&source.region).map_or(0, |ids| ids.len()) >= self.limits.max_entities_per_region
        {
            return Err(ApplyError::EntityBudgetExhausted);
        }
        let binding_changed = current.is_none_or(|(record, _, _)| record.state.binding.address != address);
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let apply_parent = current.is_none_or(|(record, _, ordering)| {
            (record.state.binding.address.region != source.region || newer_field(field_stamp, ordering.fences.parent))
                && ordering.parent_rejection_barrier.is_none_or(|barrier| barrier.admits(source, stamp))
        });
        let displaced = self.addresses.get(&address).copied().filter(|current| Some(*current) != existing);
        let mut parent_cycle = false;
        let mut parent = if apply_parent {
            match self.resolve_parent(source.region, observation.local_id, observation.parent, field_stamp, existing) {
                Ok(parent) if matches!((existing, parent), (Some(child), Parent::Resolved(parent)) if self.would_cycle(child, parent)) => {
                    parent_cycle = true;
                    Parent::Unknown
                }
                Ok(parent) => parent,
                Err(ApplyError::InvalidValue { field: "parent_cycle" }) => {
                    parent_cycle = true;
                    Parent::Unknown
                }
                Err(error) => return Err(error),
            }
        } else {
            current.expect("an existing object has a parent state").0.state.parent
        };
        if parent_cycle && binding_changed {
            return Err(ApplyError::InvalidValue { field: "parent_cycle" });
        }
        let apply_parent = apply_parent && !parent_cycle;
        if apply_parent
            && let (Some(displaced), Parent::Resolved(parent_entity), Local(parent_local_id)) = (displaced, parent, observation.parent)
            && self.parent_will_be_cascaded(displaced, parent_entity, stamp)
        {
            let expected_uuid = self.record(parent_entity).expect("resolved parent is live before the commit").id;
            parent = Parent::Pending { address: LocalAddress::new(source.region, parent_local_id), expected_uuid: Some(expected_uuid) };
        }
        if parent_cycle {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: ParentCycle });
            if observation.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some()) {
                rejections.push(FieldRejection { group: FieldGroup::Pose, reason: ParentDependsOnRejectedParent });
            }
        } else if source.authority() == Untracked
            && current.is_some_and(|(_, _, ordering)| {
                field_is_protected(source.authority(), ordering.fences.parent) || ordering.parent_rejection_barrier.is_some()
            })
        {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource });
        }
        if parent_cycle {
            observation.pose = None;
        }
        reject_untrusted_observation_groups(
            &observation,
            current.map(|(_, _, ordering)| &ordering.fences),
            source.authority(),
            &mut rejections,
        );
        let (mut current_data, mut current_content, mut current_ordering) = match current {
            Some((record, content, ordering)) => (Some(record.state), Some(content.clone()), Some(*ordering)),
            None => (None, None, None),
        };
        let next_address_generation = current_data
            .as_ref()
            .map(|data| {
                let binding = data.binding;
                if binding.address == address {
                    Ok(binding.address_generation)
                } else {
                    binding.address_generation.checked_add(1).ok_or(ApplyError::AddressGenerationExhausted)
                }
            })
            .transpose()?;
        let is_transfer = current_data.as_ref().is_some_and(|data| data.binding.address.region != source.region);
        let current_parent = current_data.as_ref().map_or(Parent::Unknown, |data| data.parent);
        let accepted_parent = apply_parent && !parent_cycle;
        let next_parent = if accepted_parent { parent } else { current_parent };
        let attachment_update_is_newer = observation.attachment_point.is_present()
            && current_ordering.as_ref().is_none_or(|ordering| newer_field(field_stamp, ordering.fences.attachment_point));
        let next_attachment_point = if attachment_update_is_newer {
            apply_field_update(current_data.as_ref().and_then(|data| data.attachment_point), observation.attachment_point)
        } else {
            current_data.as_ref().and_then(|data| data.attachment_point)
        };
        let attachment_frame = |parent: Parent| matches!(parent, Parent::Resolved(parent) if self.record(parent).is_some_and(|record| record.kind == Kind::Avatar));
        let frame_changed = is_transfer
            || (accepted_parent && parent_frame_identity_changed(self, current_parent, next_parent))
            || (attachment_update_is_newer
                && next_attachment_point != current_data.as_ref().and_then(|data| data.attachment_point)
                && (attachment_frame(current_parent) || attachment_frame(next_parent)));
        let next_frame_fence = if current_data.is_some() && frame_changed {
            Some(field_stamp)
        } else {
            current_ordering.as_ref().and_then(|ordering| ordering.fences.frame)
        };
        let next_frame_generation = current_data
            .as_ref()
            .map(|data| {
                if frame_changed {
                    data.frame_generation.checked_add(1).ok_or(ApplyError::FrameGenerationExhausted)
                } else {
                    Ok(data.frame_generation)
                }
            })
            .transpose()?;
        let pose_fences = current_ordering.as_ref().filter(|_| !is_transfer).map(|ordering| &ordering.fences);
        let pose_is_accepted = pose_is_accepted(observation.pose, field_stamp, pose_fences, next_frame_fence);
        if observation.pose.is_some_and(has_pose)
            && current_ordering.as_ref().is_some_and(|ordering| newer_field(field_stamp, ordering.fences.pose))
            && !field_at_or_after(field_stamp, next_frame_fence)
        {
            rejections.push(FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::StaleFrame });
        }
        let pose_affects_descendants = !is_transfer
            && current.is_some_and(|(record, _, ordering)| {
                pose_update_changes_observable(&record.state, ordering, observation.pose, field_stamp, next_frame_fence)
            });
        let detach_snapshot = if current_data.is_some()
            && !is_transfer
            && accepted_parent
            && next_parent == Parent::Root
            && current.is_some_and(|(record, _, _)| record.kind == Kind::Avatar)
            && matches!(current_parent, Parent::Resolved(parent) if self.record(parent).is_some_and(|record| record.kind != Kind::Avatar))
        {
            existing.and_then(|entity| match self.resolve_pose(entity).ok()? {
                PoseState::Resolved(pose) => Some(pose),
                PoseState::Unresolved(_) | PoseState::Provisional { .. } => None,
            })
        } else {
            None
        };
        let mut observation_plan = existing.map(|_| {
            let mut data = current_data.take().expect("existing object data was captured");
            let mut content = current_content.take().expect("existing object content was captured");
            let mut ordering = current_ordering.take().expect("existing object ordering was captured");
            let (before, before_content, _) = current.expect("existing entity has all components");

            if is_transfer {
                data.transform = Transform { scale: data.transform.scale, ..Transform::default() };
                data.linear_motion = LinearMotion::default();
                data.angular_velocity = None;
                data.reset_region_relative();
                content.sit_target = None;
                ordering.reset_region_relative();
                data.parent = Parent::Unknown;
            }

            if data.binding.address != address {
                data.binding = Binding {
                    address,
                    address_generation: next_address_generation.expect("existing entities have an address generation"),
                    fence: stamp,
                    parent_resolution_fence: address_was_reused.then_some(stamp),
                };
            } else {
                data.binding.fence = data.binding.fence.max(stamp);
            }

            if let Some(generation) = next_frame_generation {
                data.frame_generation = generation;
                if frame_changed {
                    data.provisional_pose = None;
                }
            }
            if frame_changed {
                ordering.fences.frame = next_frame_fence;
            }

            let parent_changed = apply_parent && data.parent != parent;
            if apply_parent {
                ordering.fences.parent = Some(field_stamp);
                ordering.parent_rejection_barrier = None;
            } else if parent_cycle && source.authority() == Tracked {
                ordering.parent_rejection_barrier = Some(ParentRejectionBarrier { region: source.region, stamp });
            }

            let surface_appearance = observation.surface_appearance.take();
            let render_materials = observation.render_materials.take();
            apply_observation_values(
                &mut data,
                &mut content,
                &mut ordering,
                &observation,
                surface_appearance,
                render_materials,
                source,
                stamp,
                pose_is_accepted,
            );
            if pose_is_accepted {
                if let Some(pose) = observation.pose {
                    mark_pose_components(&mut data, pose);
                }
                data.provisional_pose = None;
            } else if let Some(snapshot) = detach_snapshot {
                data.provisional_pose = Some(snapshot);
            }
            if source.authority() == Tracked {
                data.binding.fence = data.binding.fence.max(stamp);
            }

            let final_parent = if apply_parent { parent } else { data.parent };
            let change_classes = observable_change_classes(&before.state, before.state.parent, &data, final_parent);
            let change_classes = change_classes | content_change_classes(before_content, &content);
            ObservationPlan { data, content, ordering, parent_changed, change_classes }
        });
        let waiting_children_change = existing.is_some_and(|entity| {
            (apply_parent || binding_changed)
                && observation_plan
                    .as_ref()
                    .is_some_and(|plan| self.waiting_children_will_change(address, entity, observation.id, plan.data.binding))
        });
        if existing.is_none()
            || binding_changed
            || displaced.is_some()
            || observation_plan.as_ref().is_some_and(|plan| !plan.change_classes.is_empty())
            || waiting_children_change
        {
            self.ensure_revision_available()?;
        }
        drop(current_read);
        let mut changes = Vec::new();
        let mut recovery_targets = Vec::new();
        let mut recovery_cancellations = Vec::new();
        if let Some(displaced) = displaced {
            let mut recent = RecentBatch::default();
            self.remove_entity(
                displaced,
                RemovalCause::AddressReplaced(stamp),
                existing,
                &mut changes,
                &mut recovery_targets,
                &mut recovery_cancellations,
                &mut recent,
            );
            self.recent.apply_batch(recent);
        }

        let (entity, was_transfer, parent_changed, change_classes) = match existing {
            Some(entity) => {
                let old_binding = {
                    let current = self.record(entity).expect("UUID index contains a live entity");
                    current.state.binding
                };
                let transfer = old_binding.address.region != source.region;
                if old_binding.address != address {
                    recovery_cancellations.push(old_binding.address);
                    changes.push(if transfer {
                        EntityTransferred { id: observation.id, entity, from: old_binding.address, to: address }
                    } else {
                        EntityRebound { id: observation.id, entity, from: old_binding.address, to: address }
                    });
                    self.addresses.remove(&old_binding.address);
                    if old_binding.address.region != address.region {
                        remove_region_index(&mut self.region_entities, old_binding.address.region, observation.id);
                        self.region_entities.entry(address.region).or_default().insert(observation.id);
                    }
                    let invalidation = if transfer { old_binding.fence } else { stamp };
                    self.insert_tombstone(old_binding.address, invalidation);
                    if transfer {
                        self.pend_source_children(entity, old_binding.address, observation.id, &mut changes, &mut recovery_targets);
                        self.detach_parent(entity);
                    }
                    self.tombstones.remove(&address);
                    self.addresses.insert(address, entity);
                }
                let plan = observation_plan.take().expect("existing objects have an observation plan");
                let parent_changed = plan.parent_changed;
                let change_classes = plan.change_classes;
                self.commit_values(entity, plan.data, Some(plan.content), plan.ordering);
                (entity, transfer, parent_changed, change_classes)
            }
            None => {
                let mut data = ObjectState {
                    transform: Transform::default(),
                    binding: Binding {
                        address,
                        address_generation: 0,
                        fence: stamp,
                        parent_resolution_fence: address_was_reused.then_some(stamp),
                    },
                    parent: Parent::Unknown,
                    linear_motion: LinearMotion::default(),
                    angular_velocity: None,
                    attachment_point: None,
                    frame_generation: 0,
                    position_frame_generation: None,
                    rotation_frame_generation: None,
                    provisional_pose: None,
                };
                let mut content = ObjectContent::default();
                let mut ordering = ObjectOrdering::default();
                ordering.fences.frame = next_frame_fence;
                let surface_appearance = observation.surface_appearance.take();
                let render_materials = observation.render_materials.take();
                apply_observation_values(
                    &mut data,
                    &mut content,
                    &mut ordering,
                    &observation,
                    surface_appearance,
                    render_materials,
                    source,
                    stamp,
                    pose_is_accepted,
                );
                if let Some(pose) = observation.pose {
                    mark_pose_components(&mut data, pose);
                }
                ordering.fences.parent = Some(field_stamp);
                self.recent.remove(observation.id);
                self.tombstones.remove(&address);
                let entity = Entity(
                    self.ecs.spawn((ObjectRecord { id: observation.id, kind: observation.kind, state: data }, content, ordering)),
                    self.id,
                );
                self.entities.insert(observation.id, entity);
                if observation.kind == Kind::Avatar {
                    self.avatars.insert(observation.id);
                }
                assert!(self.object_ids.insert(observation.id), "UUID ordering index already contains a new entity ID");
                self.addresses.insert(address, entity);
                self.region_entities.entry(source.region).or_default().insert(observation.id);
                self.apply_parent(entity, parent, &mut recovery_targets);
                self.resolve_waiting_children(
                    address,
                    entity,
                    observation.id,
                    &mut changes,
                    &mut recovery_targets,
                    &mut recovery_cancellations,
                );
                changes.push(EntityAdded { id: observation.id, entity, address });
                return Ok(self
                    .finish_mutation(
                        ApplyOutcome::Introduced(entity),
                        changes,
                        Recovery::from_targets(recovery_targets, self.limits.max_recovery_targets),
                    )
                    .with_recovery_cancellations(recovery_cancellations)
                    .with_rejections(rejections)
                    .with_recovery_satisfied(true));
            }
        };

        if parent_changed {
            self.apply_parent(entity, parent, &mut recovery_targets);
        }
        if apply_parent || binding_changed {
            self.resolve_waiting_children(
                address,
                entity,
                observation.id,
                &mut changes,
                &mut recovery_targets,
                &mut recovery_cancellations,
            );
        }
        if frame_changed || parent_changed || pose_affects_descendants {
            self.invalidate_subtree(entity, &mut changes);
        }
        if !change_classes.is_empty() {
            changes.push(EntityUpdated { id: observation.id, entity, classes: change_classes });
        }
        let visible_change = !changes.is_empty();
        let outcome = if !visible_change {
            ApplyOutcome::Unchanged
        } else if self.self_agent == Some(observation.id) && was_transfer {
            ApplyOutcome::AgentHandedOff(entity)
        } else {
            ApplyOutcome::Updated(entity)
        };
        let recovery_satisfied = matches!(outcome, ApplyOutcome::Updated(_) | ApplyOutcome::Unchanged)
            && (!matches!(current_parent, Parent::Pending { .. }) || apply_parent);
        Ok(self
            .finish_mutation(outcome, changes, Recovery::from_targets(recovery_targets, self.limits.max_recovery_targets))
            .with_recovery_cancellations(recovery_cancellations)
            .with_rejections(rejections)
            .with_recovery_satisfied(recovery_satisfied))
    }

    /// Transfers the authenticated agent from `from` using destination-region evidence.
    ///
    /// This explicit operation authorizes a region change that an ordinary agent observation
    /// cannot make. Rejected independent fields are reported as for [`Self::observe_object`].
    ///
    /// # Errors
    /// Returns an [`ApplyError`] when the agent, source region, destination evidence, or required
    /// capacity is invalid. The World remains unchanged on error.
    pub fn handoff_agent(&mut self, from: Region, handoff: AgentHandoff) -> Result<Applied, ApplyError> {
        self.require_active_region(from)?;
        self.require_active_region(handoff.source.region)?;
        if from == handoff.source.region {
            return Err(ApplyError::InvalidValue { field: "handoff_region" });
        }
        let id = self.self_agent.ok_or(ApplyError::StaleRegion)?;
        let entity = self.entities.get(&id).copied().ok_or(ApplyError::StaleRegion)?;
        if self.record(entity).expect("UUID index contains a live entity").state.binding.address.region != from {
            return Err(ApplyError::StaleRegion);
        }
        self.observe_object_with_handoff(
            ObjectObservation {
                source: handoff.source,
                id,
                local_id: handoff.local_id,
                kind: Kind::Avatar,
                parent: handoff.parent,
                pose: handoff.pose,
                linear_motion: handoff.linear_motion,
                angular_velocity: handoff.angular_velocity,
                scale: handoff.scale,
                attachment_point: FieldUpdate::Keep,
                shape: FieldUpdate::Keep,
                appearance: FieldUpdate::Keep,
                surface_appearance: None,
                render_materials: None,
            },
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use base::UUID;
    use math::Vec3;

    use crate::{
        AgentHandoff, ApplyError, ApplyError::RevisionExhausted, ApplyOutcome, FieldUpdate, Kind, LinearMotion, LocalAddress,
        ObjectObservation, OrderEvidence::Ordered, ParentUpdate::Root, Pose, Region, UpdateSource, WorldRevision,
    };

    use super::super::tests::{observation, test_world};

    #[test]
    fn handoff_revision_exhaustion_is_atomic() {
        let source_region = Region::new(25);
        let destination = Region::new(26);
        let mut world = test_world(&[source_region, destination]);
        let agent_id = UUID::from_u128(25);
        world.set_self_agent(agent_id).unwrap();
        let observation = ObjectObservation {
            source: UpdateSource::new(source_region, 1, Ordered(1), 0),
            id: agent_id,
            local_id: NonZeroU32::new(8).unwrap(),
            kind: Kind::Avatar,
            parent: Root,
            pose: Some(Pose { position: Some(Vec3::new(4.0, 5.0, 6.0)), rotation: None }),
            linear_motion: Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) }),
            angular_velocity: Some(Vec3::Z),
            scale: Some(Vec3::splat(1.5)),
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Keep,
            surface_appearance: None,
            render_materials: None,
        };
        let source_local_id = observation.local_id;
        let introduced = world.observe_object(observation).unwrap();
        let ApplyOutcome::Introduced(agent) = introduced.outcome else { panic!("expected agent introduction") };
        let source_address = LocalAddress::new(source_region, source_local_id);
        let destination_address = LocalAddress::new(destination, NonZeroU32::new(9).unwrap());
        world.journal.revision = WorldRevision(u64::MAX);

        let result = world.handoff_agent(
            source_region,
            AgentHandoff {
                source: UpdateSource::new(destination, 2, Ordered(2), 0),
                local_id: destination_address.local_id(),
                parent: Root,
                pose: None,
                linear_motion: None,
                angular_velocity: None,
                scale: None,
            },
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
        assert_eq!(world.entity(UUID::from_u128(27)), Some(first));
        assert_eq!(world.entity(UUID::from_u128(28)), Some(displaced));
        assert_eq!(world.entity_at(old_address), Some(first));
        assert_eq!(world.entity_at(occupied_address), Some(displaced));
        assert_eq!(world.object_count(), 2);
    }
}
