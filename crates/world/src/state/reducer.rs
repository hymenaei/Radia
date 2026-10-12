use std::{collections::HashSet, num::NonZeroU32};

use base::UUID;
use math::{Quat, Vec3};

use super::journal::Change::{EntityAdded, EntityRebound, EntityTransferred, EntityUpdated};
use super::{
    AgentHandoff, Applied, ApplyError, ApplyOutcome, Binding, DeferredReason, Entity, EntityRemovalReason, Fences, FieldGroup,
    FieldRejection, FieldRejectionReason, FieldStamp, Identity, Kind, LinearMotion, LocalAddress, MotionPatch, ObjectData,
    ObjectObservation, ObjectPatch, Parent, ParentRejectionBarrier, ParentUpdate::Local, Pose, PoseState, Properties, PropertiesPatch,
    Recovery, Region, RegionalStamp, Rejections, SitTarget, Stamp, Transform, UpdateAuthority, UpdateSource, World, newer_field,
    newer_region_field, remove_index,
};

use FieldRejectionReason::{InvalidRotation, ParentCycle, ParentDependsOnRejectedParent, UntrustedSource};
use UpdateAuthority::{Tracked, Untracked};

#[cfg(test)]
use super::WorldLimits;

impl World {
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
                recovery(vec![LocalAddress::new(source.region, observation.local_id)], self.limits.max_recovery_targets),
            ));
        };
        let address = LocalAddress::new(source.region, observation.local_id);
        let existing = self.entities.get(&observation.id).copied();
        if source.authority() == Untracked
            && !existing.is_some_and(|entity| self.object(entity).is_some_and(|object| object.address() == address))
        {
            return Ok(self.unknown_address(address, stamp));
        }
        if let Some(t) = self.preflight_address(address, observation.id, stamp) {
            return Ok(t);
        }
        let incoming_rotation = observation.pose.and_then(|pose| pose.rotation);
        let current_rotation = existing.and_then(|entity| self.object(entity)).and_then(|object| object.rotation());
        let (mut observation, mut rejections) = validate_observation(observation);
        if let Some(pose) = observation.pose.as_mut() {
            retain_stored_orientation(current_rotation, incoming_rotation, &mut pose.rotation);
        }
        if let Some(entity) = existing {
            let (current_kind, current_binding, binding_fence, accepts_observation) = {
                let object = self.object(entity).expect("UUID index contains a live entity");
                (object.kind(), object.address(), object.data.binding.fence, object.data.accepts_observation(&observation, stamp))
            };
            if current_kind != observation.kind {
                return Err(ApplyError::KindMismatch { current: current_kind, observed: observation.kind });
            }
            if self.self_agent == Some(observation.id) && current_binding.region != source.region && !allow_agent_handoff {
                return Err(ApplyError::InvalidValue { field: "agent_handoff_required" });
            }
            if current_binding.region == source.region && current_binding != address && stamp <= binding_fence {
                return Ok(Applied::unchanged().with_rejections(rejections));
            }
            if current_binding == address && !accepts_observation {
                return Ok(Applied::unchanged().with_rejections(rejections));
            }
        } else if self.entities.len() >= self.limits.max_entities && !self.addresses.contains_key(&address) {
            return Err(ApplyError::EntityBudgetExhausted);
        }
        let address_was_reused =
            self.addresses.contains_key(&address) || self.tombstones.contains_key(&address) || self.floors.contains_key(&source.region);
        let already_in_region =
            existing.and_then(|entity| self.object(entity)).is_some_and(|object| object.address().region == source.region);
        if !already_in_region
            && !self.addresses.contains_key(&address)
            && self.region_entities.get(&source.region).map_or(0, HashSet::len) >= self.limits.max_entities_per_region
        {
            return Err(ApplyError::EntityBudgetExhausted);
        }
        let binding_changed = existing.is_none_or(|entity| self.object(entity).is_some_and(|object| object.address() != address));
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let parent_barrier_allows = |barrier: Option<ParentRejectionBarrier>| match barrier {
            Some(barrier) if barrier.region != source.region => true,
            Some(barrier) => match source.authority() {
                Tracked => stamp > barrier.stamp,
                Untracked => false,
            },
            None => true,
        };
        let apply_parent = existing.is_none_or(|entity| {
            let object = self.object(entity).expect("UUID index contains a live entity");
            (object.address().region != source.region || newer_field(field_stamp, object.data.fences.parent))
                && parent_barrier_allows(object.data.parent_rejection_barrier)
        });
        let displaced = self.addresses.get(&address).copied().filter(|current| Some(*current) != existing);
        let before_observable = existing.map(|entity| self.observable_state(entity));
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
            self.object(existing.expect("an existing object has a parent state")).expect("UUID index contains a live entity").parent()
        };
        if parent_cycle && binding_changed {
            return Err(ApplyError::InvalidValue { field: "parent_cycle" });
        }
        let apply_parent = apply_parent && !parent_cycle;
        if apply_parent
            && let (Some(displaced), Parent::Resolved(parent_entity), Local(parent_local_id)) = (displaced, parent, observation.parent)
            && self.parent_will_be_cascaded(displaced, parent_entity, stamp)
        {
            let expected_uuid = self.object(parent_entity).expect("resolved parent is live before the commit").identity.id;
            parent = Parent::Pending { address: LocalAddress::new(source.region, parent_local_id), expected_uuid: Some(expected_uuid) };
        }
        if parent_cycle {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: ParentCycle });
            if observation.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some()) {
                rejections.push(FieldRejection { group: FieldGroup::Pose, reason: ParentDependsOnRejectedParent });
            }
        } else if source.authority() == Untracked
            && existing.is_some_and(|entity| {
                self.object(entity).is_some_and(|object| {
                    field_is_protected(source.authority(), object.data.fences.parent) || object.data.parent_rejection_barrier.is_some()
                })
            })
        {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource });
        }
        if parent_cycle {
            observation.pose = None;
        }
        reject_untrusted_observation_groups(&observation, existing, self, source.authority(), &mut rejections);
        let current_data = existing.and_then(|entity| self.object(entity)).map(|object| *object.data);
        let is_transfer = current_data.is_some_and(|data| data.binding.address.region != source.region);
        let current_parent = current_data.map_or(Parent::Unknown, |data| data.parent);
        let accepted_parent = apply_parent && !parent_cycle;
        let next_parent = if accepted_parent { parent } else { current_parent };
        let attachment_update_is_newer = observation.attachment_point.is_some()
            && current_data.is_none_or(|data| newer_field(field_stamp, data.fences.attachment_point));
        let next_attachment_point = if attachment_update_is_newer {
            observation.attachment_point.expect("accepted attachment point update is present")
        } else {
            current_data.and_then(|data| data.attachment_point)
        };
        let attachment_frame = |parent: Parent| matches!(parent, Parent::Resolved(parent) if self.object(parent).is_some_and(|object| object.kind() == Kind::Avatar));
        let frame_changed = is_transfer
            || (accepted_parent && parent_frame_identity_changed(self, current_parent, next_parent))
            || (attachment_update_is_newer
                && next_attachment_point != current_data.and_then(|data| data.attachment_point)
                && (attachment_frame(current_parent) || attachment_frame(next_parent)));
        let next_frame_generation = current_data
            .map(|data| {
                if frame_changed {
                    data.frame_generation.checked_add(1).ok_or(ApplyError::FrameGenerationExhausted)
                } else {
                    Ok(data.frame_generation)
                }
            })
            .transpose()?;
        let pose_is_accepted = observation.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some())
            && (is_transfer || current_data.is_none_or(|data| newer_field(field_stamp, data.fences.pose)));
        let pose_affects_descendants = existing.is_some_and(|entity| {
            self.object(entity)
                .is_some_and(|object| pose_update_changes_observable(&object.data, object.transform(), observation.pose, field_stamp))
        });
        let detach_snapshot = if current_data.is_some()
            && !is_transfer
            && accepted_parent
            && next_parent == Parent::Root
            && existing.and_then(|entity| self.object(entity)).is_some_and(|object| object.kind() == Kind::Avatar)
            && matches!(current_parent, Parent::Resolved(parent) if self.object(parent).is_some_and(|object| object.kind() != Kind::Avatar))
        {
            existing.and_then(|entity| self.object(entity)).and_then(|object| match object.world_pose().ok()? {
                PoseState::Resolved(pose) => Some(pose),
                PoseState::Unresolved(_) | PoseState::Provisional { .. } => None,
            })
        } else {
            None
        };
        if self.observation_changes_observable(&observation, existing, address, stamp, apply_parent, parent, source) {
            self.ensure_revision_available()?;
        }
        let mut changes = Vec::new();
        let mut recovery_targets = Vec::new();
        let mut recovery_cancellations = Vec::new();
        if let Some(displaced) = displaced {
            self.remove_entity(
                displaced,
                Some(stamp),
                false,
                EntityRemovalReason::AddressReplaced,
                existing,
                &mut changes,
                &mut recovery_targets,
                &mut recovery_cancellations,
            )?;
        }

        let (entity, was_transfer) = match existing {
            Some(entity) => {
                let (old_binding, previous_transform) = {
                    let current = self.object(entity).expect("UUID index contains a live entity");
                    (current.data.binding, current.transform())
                };
                let transfer = old_binding.address.region != source.region;
                if old_binding.address != address {
                    recovery_cancellations.push(old_binding.address);
                    changes.push(if transfer {
                        EntityTransferred { id: observation.id, from: old_binding.address, to: address }
                    } else {
                        EntityRebound { id: observation.id, from: old_binding.address, to: address }
                    });
                    self.addresses.remove(&old_binding.address);
                    if old_binding.address.region != address.region {
                        remove_index(&mut self.region_entities, old_binding.address.region, entity);
                        self.region_entities.entry(address.region).or_default().insert(entity);
                    }
                    let invalidation = if transfer { old_binding.fence } else { stamp };
                    self.insert_tombstone(old_binding.address, invalidation);
                    if transfer {
                        self.pend_source_children(entity, old_binding.address, observation.id, &mut changes, &mut recovery_targets);
                        self.detach_parent(entity);
                    }
                    self.tombstones.remove(&address);
                    self.addresses.insert(address, entity);
                    if transfer {
                        *self.transform_mut(entity) = Transform { scale: previous_transform.scale, ..Transform::default() };
                        let mut data = self.data_mut(entity);
                        data.linear_motion = LinearMotion::default();
                        data.angular_velocity = None;
                        data.reset_region_relative();
                    }
                    let mut data = self.data_mut(entity);
                    data.binding = Binding { address, fence: stamp, parent_resolution_fence: address_was_reused.then_some(stamp) };
                    if transfer {
                        data.parent = Parent::Unknown;
                    }
                } else {
                    let mut data = self.data_mut(entity);
                    data.binding.fence = data.binding.fence.max(stamp);
                }
                (entity, transfer)
            }
            None => {
                let identity = Identity { id: observation.id };
                let mut transform = Transform::default();
                let mut data = ObjectData {
                    binding: Binding { address, fence: stamp, parent_resolution_fence: address_was_reused.then_some(stamp) },
                    parent: Parent::Unknown,
                    parent_rejection_barrier: None,
                    linear_motion: LinearMotion::default(),
                    angular_velocity: None,
                    shape: None,
                    appearance: None,
                    attachment_point: None,
                    sit_target: None,
                    frame_generation: 0,
                    position_frame_generation: None,
                    rotation_frame_generation: None,
                    provisional_pose: None,
                    properties: Properties::default(),
                    fences: Fences::default(),
                };
                apply_observation_values(&mut data, &mut transform, &observation, source, stamp);
                if let Some(pose) = observation.pose {
                    mark_pose_components(&mut data, pose);
                }
                data.fences.parent = Some(field_stamp);
                self.tombstones.remove(&address);
                let entity = Entity(self.ecs.spawn((identity, observation.kind, transform, data)), self.id);
                self.entities.insert(observation.id, entity);
                self.addresses.insert(address, entity);
                self.region_entities.entry(source.region).or_default().insert(entity);
                self.apply_parent(entity, parent, &mut recovery_targets);
                self.resolve_waiting_children(
                    address,
                    entity,
                    observation.id,
                    &mut changes,
                    &mut recovery_targets,
                    &mut recovery_cancellations,
                );
                changes.push(EntityAdded { id: observation.id, address });
                return Ok(self
                    .finish(ApplyOutcome::Introduced(entity), changes, recovery(recovery_targets, self.limits.max_recovery_targets))
                    .with_recovery_cancellations(recovery_cancellations)
                    .with_rejections(rejections));
            }
        };

        let mut transform = *self.transform(entity);
        let mut data = self.data_mut(entity);
        if was_transfer {
            data.parent = Parent::Unknown;
            data.binding = Binding { address, fence: stamp, parent_resolution_fence: address_was_reused.then_some(stamp) };
        }
        if let Some(generation) = next_frame_generation {
            data.frame_generation = generation;
            if frame_changed {
                data.provisional_pose = None;
            }
        }
        let parent_changed = apply_parent && data.parent != parent;
        if apply_parent {
            data.fences.parent = Some(field_stamp);
            data.parent_rejection_barrier = None;
        } else if parent_cycle && source.authority() == Tracked {
            data.parent_rejection_barrier = Some(ParentRejectionBarrier { region: source.region, stamp });
        }
        apply_observation_values(&mut data, &mut transform, &observation, source, stamp);
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
        drop(data);
        *self.transform_mut(entity) = transform;
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
        let object_changed = before_observable.is_none_or(|before| self.observable_state(entity) != before);
        if object_changed {
            changes.push(EntityUpdated(observation.id));
        }
        let visible_change = !changes.is_empty();
        Ok(self
            .finish(
                if !visible_change {
                    ApplyOutcome::Unchanged
                } else if self.self_agent == Some(observation.id) && was_transfer {
                    ApplyOutcome::AgentHandedOff(entity)
                } else {
                    ApplyOutcome::Updated(entity)
                },
                changes,
                recovery(recovery_targets, self.limits.max_recovery_targets),
            )
            .with_recovery_cancellations(recovery_cancellations)
            .with_rejections(rejections))
    }

    pub fn handoff_agent(&mut self, from: Region, handoff: AgentHandoff) -> Result<Applied, ApplyError> {
        self.require_active_region(from)?;
        self.require_active_region(handoff.source.region)?;
        if from == handoff.source.region {
            return Err(ApplyError::InvalidValue { field: "handoff_region" });
        }
        let id = self.self_agent.ok_or(ApplyError::StaleRegion)?;
        let entity = self.entities.get(&id).copied().ok_or(ApplyError::StaleRegion)?;
        if self.object(entity).expect("UUID index contains a live entity").address().region != from {
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
                attachment_point: None,
            },
            true,
        )
    }

    pub fn patch_object(&mut self, patch: ObjectPatch) -> Result<Applied, ApplyError> {
        let source = patch.source;
        let local_id = patch.local_id;
        self.require_active_region(source.region)?;
        let Some(stamp) = source.stamp() else {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                recovery(vec![LocalAddress::new(source.region, local_id)], self.limits.max_recovery_targets),
            ));
        };
        let address = LocalAddress::new(source.region, local_id);
        let Some(entity) = self.addresses.get(&address).copied() else {
            return Ok(self.unknown_address(address, stamp));
        };
        let has_update = patch.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some())
            || patch.linear_motion.velocity.is_some()
            || patch.linear_motion.acceleration.is_some()
            || patch.angular_velocity.is_some()
            || patch.scale.is_some()
            || patch.parent.is_some()
            || patch.attachment_point.is_some();
        let (data, transform, kind, object_parent) = {
            let object = self.object(entity).expect("address index contains a live entity");
            (*object.data, object.transform(), object.kind(), object.parent())
        };
        let incoming_rotation = patch.pose.and_then(|pose| pose.rotation);
        let current_rotation = transform.rotation();
        let (mut patch, mut rejections) = validate_patch(patch);
        if let Some(pose) = patch.pose.as_mut() {
            retain_stored_orientation(current_rotation, incoming_rotation, &mut pose.rotation);
        }
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let parent_candidate = patch.parent.is_some()
            && newer_field(field_stamp, data.fences.parent)
            && match data.parent_rejection_barrier {
                Some(barrier) if barrier.region != source.region => true,
                Some(barrier) => match source.authority() {
                    Tracked => stamp > barrier.stamp,
                    Untracked => false,
                },
                None => true,
            };
        let mut parent_cycle = false;
        let parent = if parent_candidate {
            let update = patch.parent.expect("candidate parent update is present");
            match self.resolve_parent(source.region, local_id, update, field_stamp, Some(entity)) {
                Ok(Parent::Resolved(parent)) if self.would_cycle(entity, parent) => {
                    parent_cycle = true;
                    None
                }
                Ok(parent) => Some(parent),
                Err(ApplyError::InvalidValue { field: "parent_cycle" }) => {
                    parent_cycle = true;
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        if parent_cycle {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: ParentCycle });
            if patch.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some()) {
                rejections.push(FieldRejection { group: FieldGroup::Pose, reason: ParentDependsOnRejectedParent });
                patch.pose = None;
            }
        } else if source.authority() == Untracked
            && patch.parent.is_some()
            && (data.fences.parent.is_some_and(|fence| fence.authority == Tracked) || data.parent_rejection_barrier.is_some())
        {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource });
        }
        reject_untrusted_patch_groups(&patch, data.fences, source.authority(), &mut rejections);
        let parent_is_newer = parent_candidate && !parent_cycle;
        let next_parent = if parent_is_newer { parent.expect("newer parent update was resolved") } else { data.parent };
        let attachment_update_is_newer = patch.attachment_point.is_some() && newer_field(field_stamp, data.fences.attachment_point);
        let next_attachment_point = if attachment_update_is_newer {
            patch.attachment_point.expect("accepted attachment point update is present")
        } else {
            data.attachment_point
        };
        let attachment_frame = |parent: Parent| matches!(parent, Parent::Resolved(parent) if self.object(parent).is_some_and(|object| object.kind() == Kind::Avatar));
        let frame_changed = (parent_is_newer && parent_frame_identity_changed(self, data.parent, next_parent))
            || (attachment_update_is_newer
                && next_attachment_point != data.attachment_point
                && (attachment_frame(data.parent) || attachment_frame(next_parent)));
        let next_frame_generation = if frame_changed {
            data.frame_generation.checked_add(1).ok_or(ApplyError::FrameGenerationExhausted)?
        } else {
            data.frame_generation
        };
        let pose_is_accepted =
            patch.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some()) && newer_field(field_stamp, data.fences.pose);
        let pose_affects_descendants = pose_update_changes_observable(&data, transform, patch.pose, field_stamp);
        let detach_snapshot = if parent_is_newer
            && next_parent == Parent::Root
            && kind == Kind::Avatar
            && matches!(data.parent, Parent::Resolved(parent) if self.object(parent).is_some_and(|object| object.kind() != Kind::Avatar))
        {
            match self.object(entity).and_then(|object| object.world_pose().ok()) {
                Some(PoseState::Resolved(pose)) => Some(pose),
                Some(PoseState::Unresolved(_) | PoseState::Provisional { .. }) | None => None,
            }
        } else {
            None
        };
        let parent_changed = parent_is_newer && object_parent != next_parent;
        let observable_change = pose_affects_descendants
            || ((patch.linear_motion.velocity.is_some() || patch.linear_motion.acceleration.is_some())
                && newer_field(field_stamp, data.fences.linear_motion)
                && (patch.linear_motion.velocity.is_some_and(|velocity| data.linear_motion.velocity != Some(velocity))
                    || patch.linear_motion.acceleration.is_some_and(|acceleration| data.linear_motion.acceleration != Some(acceleration))))
            || patch
                .angular_velocity
                .is_some_and(|value| newer_field(field_stamp, data.fences.angular_velocity) && data.angular_velocity != Some(value))
            || patch.scale.is_some_and(|value| newer_field(field_stamp, data.fences.scale) && transform.scale() != Some(value))
            || (attachment_update_is_newer && data.attachment_point != next_attachment_point)
            || parent_changed;
        if observable_change {
            self.ensure_revision_available()?;
        }
        let mut changed = false;
        let mut transform = *self.transform(entity);
        let mut data = self.data_mut(entity);
        changed |= parent_changed;
        data.frame_generation = next_frame_generation;
        if frame_changed {
            data.provisional_pose = None;
        }
        if let Some(pose) = patch.pose
            && (pose.position.is_some() || pose.rotation.is_some())
            && newer_field(field_stamp, data.fences.pose)
        {
            if let Some(position) = pose.position {
                changed |= transform.position != Some(position);
                transform.position = Some(position);
            }
            if let Some(rotation) = pose.rotation {
                changed |= rotation_changes(transform.rotation, rotation);
                if !transform.rotation.is_some_and(|current| is_exact_negation(current, rotation)) {
                    transform.rotation = Some(rotation);
                }
            }
            data.fences.pose = Some(field_stamp);
        }
        if (patch.linear_motion.velocity.is_some() || patch.linear_motion.acceleration.is_some())
            && newer_field(field_stamp, data.fences.linear_motion)
        {
            changed |= patch_motion(&mut data.linear_motion, patch.linear_motion);
            data.fences.linear_motion = Some(field_stamp);
        }
        if let Some(value) = patch.angular_velocity
            && newer_field(field_stamp, data.fences.angular_velocity)
        {
            changed |= data.angular_velocity != Some(value);
            data.angular_velocity = Some(value);
            data.fences.angular_velocity = Some(field_stamp);
        }
        if let Some(value) = patch.scale
            && newer_field(field_stamp, data.fences.scale)
        {
            changed |= transform.scale != Some(value);
            transform.scale = Some(value);
            data.fences.scale = Some(field_stamp);
        }
        if let Some(value) = patch.attachment_point
            && newer_field(field_stamp, data.fences.attachment_point)
        {
            changed |= data.attachment_point != value;
            data.attachment_point = value;
            data.fences.attachment_point = Some(field_stamp);
        }
        if pose_is_accepted {
            if let Some(pose) = patch.pose {
                mark_pose_components(&mut data, pose);
            }
            data.provisional_pose = None;
        } else if let Some(snapshot) = detach_snapshot {
            data.provisional_pose = Some(snapshot);
        }
        if parent_is_newer {
            data.fences.parent = Some(field_stamp);
            data.parent_rejection_barrier = None;
        } else if parent_cycle && source.authority() == Tracked {
            data.parent_rejection_barrier = Some(ParentRejectionBarrier { region: source.region, stamp });
        }
        if has_update && source.authority() == Tracked {
            data.binding.fence = data.binding.fence.max(stamp);
        }
        drop(data);
        *self.transform_mut(entity) = transform;
        let mut changes = Vec::new();
        let mut recovery_targets = Vec::new();
        changed |= pose_affects_descendants;
        if let Some(parent) = parent.filter(|_| parent_is_newer) {
            self.apply_parent(entity, parent, &mut recovery_targets);
        }
        let id = self.object(entity).expect("address index contains a live entity").identity.id;
        if changed {
            changes.push(EntityUpdated(id));
        }
        if frame_changed || parent_changed || pose_affects_descendants {
            self.invalidate_subtree(entity, &mut changes);
        }
        Ok(self
            .finish(
                if changed { ApplyOutcome::Updated(entity) } else { ApplyOutcome::Unchanged },
                changes,
                recovery(recovery_targets, self.limits.max_recovery_targets),
            )
            .with_rejections(rejections))
    }

    pub fn patch_properties(&mut self, source: UpdateSource, id: UUID, patch: PropertiesPatch) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        validate_uuid(id, "object_id")?;
        let Some(stamp) = source.stamp() else {
            let recovery_address = self
                .entities
                .get(&id)
                .and_then(|entity| self.object(*entity))
                .map(|object| object.address())
                .filter(|address| address.region == source.region);
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                recovery_address.and_then(|address| recovery(vec![address], self.limits.max_recovery_targets)),
            ));
        };
        let Some(entity) = self.entities.get(&id).copied() else {
            return Ok(Applied::deferred(DeferredReason::UnknownObject, None));
        };
        let address = self.object(entity).expect("UUID index contains a live entity").address();
        if address.region != source.region {
            return Ok(Applied::unchanged());
        }
        let has_property_update = patch.owner_id.is_some() || patch.creator_id.is_some();
        let (properties, property_fence) = {
            let object = self.object(entity).expect("UUID index contains a live entity");
            (object.data.properties, object.data.fences.properties)
        };
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let mut rejections = Rejections::default();
        let untrusted_overwrite = source.authority() == Untracked && property_fence.is_some_and(|fence| fence.stamp.authority == Tracked);
        if has_property_update && untrusted_overwrite {
            rejections.push(FieldRejection { group: FieldGroup::Properties, reason: UntrustedSource });
        }
        let properties_is_newer =
            has_property_update && !untrusted_overwrite && newer_region_field(source.region, field_stamp, property_fence);
        let observable_change = properties_is_newer
            && (patch.owner_id.is_some_and(|value| properties.owner_id != Some(value))
                || patch.creator_id.is_some_and(|value| properties.creator_id != Some(value)));
        if observable_change {
            self.ensure_revision_available()?;
        }
        let mut changed = false;
        {
            let mut data = self.data_mut(entity);
            if properties_is_newer {
                if let Some(value) = patch.owner_id {
                    changed |= data.properties.owner_id != Some(value);
                    data.properties.owner_id = Some(value);
                }
                if let Some(value) = patch.creator_id {
                    changed |= data.properties.creator_id != Some(value);
                    data.properties.creator_id = Some(value);
                }
                data.fences.properties = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if has_property_update && source.authority() == Tracked {
                data.binding.fence = data.binding.fence.max(stamp);
            }
        }
        let changes = if changed { vec![EntityUpdated(id)] } else { Vec::new() };
        Ok(self
            .finish(if changed { ApplyOutcome::Updated(entity) } else { ApplyOutcome::Unchanged }, changes, None)
            .with_rejections(rejections))
    }

    pub fn patch_sit_target(&mut self, source: UpdateSource, id: UUID, target: Option<SitTarget>) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        validate_uuid(id, "object_id")?;
        let Some(stamp) = source.stamp() else {
            let recovery_address = self
                .entities
                .get(&id)
                .and_then(|entity| self.object(*entity))
                .map(|object| object.address())
                .filter(|address| address.region == source.region);
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                recovery_address.and_then(|address| recovery(vec![address], self.limits.max_recovery_targets)),
            ));
        };
        let Some(entity) = self.entities.get(&id).copied() else {
            return Ok(Applied::deferred(DeferredReason::UnknownObject, None));
        };
        let object = self.object(entity).expect("UUID index contains a live entity");
        if object.address().region != source.region {
            return Ok(Applied::unchanged());
        }

        let mut rejections = Rejections::default();
        let target = match target {
            Some(mut target) => {
                let invalid_position = validate_vec3(Some(target.position), "sit_position").err();
                let rotation = normalized_rotation(target.rotation);
                if let Some(reason) = invalid_position.or_else(|| rotation.as_ref().err().copied()) {
                    rejections.push(FieldRejection { group: FieldGroup::SitTarget, reason });
                    return Ok(Applied::unchanged().with_rejections(rejections));
                }
                target.rotation = rotation.expect("sit-target rotation was validated");
                Some(target)
            }
            None => None,
        };

        let (sit_target, sit_target_fence) = (object.data.sit_target, object.data.fences.sit_target);
        drop(object);
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let protected = source.authority() == Untracked && sit_target_fence.is_some_and(|fence| fence.stamp.authority == Tracked);
        if protected {
            rejections.push(FieldRejection { group: FieldGroup::SitTarget, reason: UntrustedSource });
        }
        let accepted = !protected && newer_region_field(source.region, field_stamp, sit_target_fence);
        let observable_change = accepted && sit_target != target;
        if observable_change {
            self.ensure_revision_available()?;
        }
        if !accepted {
            return Ok(Applied::unchanged().with_rejections(rejections));
        }

        {
            let mut data = self.data_mut(entity);
            data.sit_target = target;
            data.fences.sit_target = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            if source.authority() == Tracked {
                data.binding.fence = data.binding.fence.max(stamp);
            }
        }
        let changes = if observable_change { vec![EntityUpdated(id)] } else { Vec::new() };
        Ok(self
            .finish(if observable_change { ApplyOutcome::Updated(entity) } else { ApplyOutcome::Unchanged }, changes, None)
            .with_rejections(rejections))
    }

    pub fn remove_object(&mut self, source: UpdateSource, local_id: NonZeroU32) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        if source.authority() == Untracked {
            return Ok(Applied::unchanged());
        }
        let Some(stamp) = source.stamp() else {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                recovery(vec![LocalAddress::new(source.region, local_id)], self.limits.max_recovery_targets),
            ));
        };
        let address = LocalAddress::new(source.region, local_id);
        if let Some(entity) = self.addresses.get(&address).copied() {
            let (id, binding_fence) = {
                let object = self.object(entity).expect("address index contains a live entity");
                (object.identity.id, object.data.binding.fence)
            };
            if stamp <= binding_fence {
                return Ok(Applied::unchanged());
            }
            self.ensure_revision_available()?;
            let mut changes = Vec::new();
            let mut recovery_targets = Vec::new();
            let mut recovery_cancellations = Vec::new();
            self.remove_entity(
                entity,
                Some(stamp),
                true,
                EntityRemovalReason::Killed,
                None,
                &mut changes,
                &mut recovery_targets,
                &mut recovery_cancellations,
            )?;
            return Ok(self
                .finish(ApplyOutcome::Removed(id), changes, recovery(recovery_targets, self.limits.max_recovery_targets))
                .with_recovery_cancellations(recovery_cancellations));
        }
        if let Some(tombstone) = self.tombstones.get(&address) {
            if stamp <= *tombstone {
                return Ok(Applied::unchanged());
            }
        } else if self.floors.get(&source.region).is_some_and(|floor| stamp <= *floor) {
            return Ok(Applied::deferred(DeferredReason::AmbiguousOrder, recovery(vec![address], self.limits.max_recovery_targets)));
        }
        if !self.tombstone_would_change(address, stamp) {
            return Ok(Applied::unchanged());
        }
        if self.insert_tombstone(address, stamp) {
            return Ok(self.finish(ApplyOutcome::TombstoneRecorded(address), Vec::new(), None));
        }
        Ok(Applied::unchanged())
    }

    fn preflight_address(&self, address: LocalAddress, id: UUID, stamp: Stamp) -> Option<Applied> {
        if let Some(entity) = self.addresses.get(&address) {
            let object = self.object(*entity).expect("address index contains a live entity");
            if object.identity.id != id && stamp <= object.data.binding.fence {
                return Some(Applied::unchanged());
            }
            return None;
        }
        if let Some(tombstone) = self.tombstones.get(&address) {
            return if stamp <= *tombstone { Some(Applied::unchanged()) } else { None };
        }
        if self.floors.get(&address.region).is_some_and(|floor| stamp <= *floor) {
            return Some(Applied::deferred(DeferredReason::AmbiguousOrder, recovery(vec![address], self.limits.max_recovery_targets)));
        }
        None
    }

    fn parent_will_be_cascaded(&self, root: Entity, target: Entity, stamp: Stamp) -> bool {
        let Some(root) = self.object(root) else { return false };
        let root_region = root.address().region();
        let mut current = target;

        for _ in 0..=self.entities.len() {
            if current == root.entity() {
                return true;
            }

            let Some(object) = self.object(current) else { return false };
            if object.kind() == Kind::Avatar || object.address().region() != root_region || stamp <= object.data.binding.fence {
                return false;
            }

            let Parent::Resolved(parent) = object.parent() else { return false };
            current = parent;
        }

        false
    }

    fn observation_changes_observable(
        &self,
        observation: &ObjectObservation,
        existing: Option<Entity>,
        address: LocalAddress,
        stamp: Stamp,
        apply_parent: bool,
        parent: Parent,
        source: UpdateSource,
    ) -> bool {
        let Some(entity) = existing else {
            return true;
        };
        if self.addresses.get(&address).is_some_and(|occupant| *occupant != entity) {
            return true;
        }

        let object = self.object(entity).expect("UUID index contains a live entity");
        if object.address() != address {
            return true;
        }
        let data = *object.data;
        let transform = object.transform();
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let observation_changes_fields = pose_update_changes_observable(&data, transform, observation.pose, field_stamp)
            || observation.linear_motion.is_some_and(|motion| {
                (motion.velocity.is_some() || motion.acceleration.is_some())
                    && newer_field(field_stamp, data.fences.linear_motion)
                    && (motion.velocity.is_some_and(|velocity| data.linear_motion.velocity != Some(velocity))
                        || motion.acceleration.is_some_and(|acceleration| data.linear_motion.acceleration != Some(acceleration)))
            })
            || observation
                .angular_velocity
                .is_some_and(|value| newer_field(field_stamp, data.fences.angular_velocity) && data.angular_velocity != Some(value))
            || observation.scale.is_some_and(|value| newer_field(field_stamp, data.fences.scale) && transform.scale() != Some(value))
            || observation
                .attachment_point
                .is_some_and(|value| newer_field(field_stamp, data.fences.attachment_point) && data.attachment_point != value);

        if observation_changes_fields || (apply_parent && object.parent() != parent) {
            return true;
        }

        if !apply_parent && object.address() == address {
            return false;
        }
        let parent_binding = data.binding;
        self.pending_children.get(&address).is_some_and(|waiting| {
            waiting.iter().any(|child| {
                self.object(*child).is_some_and(|child_object| {
                    let parent_stamp = child_object.data.fences.parent;
                    match child_object.parent() {
                        Parent::Pending { expected_uuid, .. } if expected_uuid.is_none_or(|expected| expected == observation.id) => {
                            parent_binding.admits_parent_link(expected_uuid, parent_stamp, observation.id)
                                && !self.would_cycle(*child, entity)
                        }
                        _ => false,
                    }
                })
            })
        })
    }

    fn unknown_address(&self, address: LocalAddress, stamp: Stamp) -> Applied {
        if let Some(tombstone) = self.tombstones.get(&address) {
            if stamp <= *tombstone {
                return Applied::unchanged();
            }
            return Applied::deferred(DeferredReason::UnknownObject, recovery(vec![address], self.limits.max_recovery_targets));
        }
        if self.floors.get(&address.region).is_some_and(|floor| stamp <= *floor) {
            return Applied::deferred(DeferredReason::AmbiguousOrder, recovery(vec![address], self.limits.max_recovery_targets));
        }
        Applied::deferred(DeferredReason::UnknownObject, recovery(vec![address], self.limits.max_recovery_targets))
    }
}
fn validate_uuid(id: UUID, field: &'static str) -> Result<(), ApplyError> {
    if id.is_nil() { Err(ApplyError::InvalidValue { field }) } else { Ok(()) }
}

fn pose_frame_markers_change(data: &ObjectData, pose: Pose) -> bool {
    (pose.position.is_some() && data.position_frame_generation != Some(data.frame_generation))
        || (pose.rotation.is_some() && data.rotation_frame_generation != Some(data.frame_generation))
}

fn pose_update_changes_observable(data: &ObjectData, transform: Transform, pose: Option<Pose>, stamp: FieldStamp) -> bool {
    let Some(pose) = pose.filter(|pose| pose.position.is_some() || pose.rotation.is_some()) else { return false };
    if !newer_field(stamp, data.fences.pose) {
        return false;
    }
    data.provisional_pose.is_some()
        || pose_frame_markers_change(data, pose)
        || pose.position.is_some_and(|position| transform.position() != Some(position))
        || pose.rotation.is_some_and(|rotation| rotation_changes(transform.rotation(), rotation))
}

fn mark_pose_components(data: &mut ObjectData, pose: Pose) {
    if pose.position.is_some() {
        data.position_frame_generation = Some(data.frame_generation);
    }
    if pose.rotation.is_some() {
        data.rotation_frame_generation = Some(data.frame_generation);
    }
}

fn parent_frame_identity_changed(world: &World, current: Parent, next: Parent) -> bool {
    if current == next {
        return false;
    }
    match (current, next) {
        (Parent::Pending { address, expected_uuid }, Parent::Resolved(entity))
        | (Parent::Resolved(entity), Parent::Pending { address, expected_uuid }) => world.object(entity).is_none_or(|object| {
            object.address() != address
                || expected_uuid
                    .map_or(object.data.binding.parent_resolution_fence.is_some(), |expected| expected != object.identity().id())
        }),
        _ => true,
    }
}

fn validate_vec3(value: Option<Vec3>, field: &'static str) -> Result<(), FieldRejectionReason> {
    if value.is_some_and(|value| !value.is_finite()) { Err(FieldRejectionReason::InvalidValue { field }) } else { Ok(()) }
}

fn normalized_rotation(value: Quat) -> Result<Quat, FieldRejectionReason> {
    // Quantization contributes the first-order bound. Zero-snapping adds only a quadratic term because it affects
    // components within two steps of zero; the float allowance covers that term and XYZ reconstruction rounding.
    const U16_SQUARED_NORM_ERROR: f32 = 4.0 / 65_535.0 + 48.0 / (65_535.0 * 65_535.0);
    const F32_ROUNDING_ERROR: f32 = 8.0 * f32::EPSILON;
    const NORM_SQUARED_TOLERANCE: f32 = U16_SQUARED_NORM_ERROR + F32_ROUNDING_ERROR;

    let norm_squared = value.length_squared();
    if !value.is_finite()
        || !norm_squared.is_finite()
        || norm_squared <= f32::EPSILON
        || (norm_squared - 1.0).abs() > NORM_SQUARED_TOLERANCE
    {
        Err(InvalidRotation)
    } else {
        Ok(value.normalize())
    }
}

fn validate_observation(mut observation: ObjectObservation) -> (ObjectObservation, Rejections) {
    let mut rejections = Rejections::default();
    if let Some(mut pose) = observation.pose
        && (pose.position.is_some() || pose.rotation.is_some())
    {
        let invalid_position = validate_vec3(pose.position, "position").err();
        let rotation = pose.rotation.map(normalized_rotation).transpose();
        let invalid_rotation = rotation.as_ref().err().copied();
        if let Some(reason) = invalid_position.or(invalid_rotation) {
            rejections.push(FieldRejection { group: FieldGroup::Pose, reason });
            observation.pose = None;
        } else {
            pose.rotation = rotation.expect("rotation was validated");
            observation.pose = Some(pose);
        }
    }
    if let Some(motion) = observation.linear_motion
        && (motion.velocity.is_some() || motion.acceleration.is_some())
    {
        let reason = validate_vec3(motion.velocity, "velocity").err().or_else(|| validate_vec3(motion.acceleration, "acceleration").err());
        if let Some(reason) = reason {
            rejections.push(FieldRejection { group: FieldGroup::LinearMotion, reason });
            observation.linear_motion = None;
        }
    }
    if let Some(reason) = validate_vec3(observation.angular_velocity, "angular_velocity").err() {
        rejections.push(FieldRejection { group: FieldGroup::AngularVelocity, reason });
        observation.angular_velocity = None;
    }
    if let Some(reason) = validate_vec3(observation.scale, "scale").err() {
        rejections.push(FieldRejection { group: FieldGroup::Scale, reason });
        observation.scale = None;
    }
    (observation, rejections)
}

fn validate_patch(mut patch: ObjectPatch) -> (ObjectPatch, Rejections) {
    let mut rejections = Rejections::default();
    if let Some(mut pose) = patch.pose
        && (pose.position.is_some() || pose.rotation.is_some())
    {
        let invalid_position = validate_vec3(pose.position, "position").err();
        let rotation = pose.rotation.map(normalized_rotation).transpose();
        let invalid_rotation = rotation.as_ref().err().copied();
        if let Some(reason) = invalid_position.or(invalid_rotation) {
            rejections.push(FieldRejection { group: FieldGroup::Pose, reason });
            patch.pose = None;
        } else {
            pose.rotation = rotation.expect("rotation was validated");
            patch.pose = Some(pose);
        }
    }
    if let Some(reason) = validate_vec3(patch.linear_motion.velocity, "velocity")
        .err()
        .or_else(|| validate_vec3(patch.linear_motion.acceleration, "acceleration").err())
    {
        rejections.push(FieldRejection { group: FieldGroup::LinearMotion, reason });
        patch.linear_motion = MotionPatch::default();
    }
    if let Some(reason) = validate_vec3(patch.angular_velocity, "angular_velocity").err() {
        rejections.push(FieldRejection { group: FieldGroup::AngularVelocity, reason });
        patch.angular_velocity = None;
    }
    if let Some(reason) = validate_vec3(patch.scale, "scale").err() {
        rejections.push(FieldRejection { group: FieldGroup::Scale, reason });
        patch.scale = None;
    }
    (patch, rejections)
}

fn rotation_changes(current: Option<Quat>, next: Quat) -> bool {
    current.is_none_or(|current| current != next && !is_exact_negation(current, next))
}

fn retain_stored_orientation(current: Option<Quat>, incoming: Option<Quat>, next: &mut Option<Quat>) {
    let Some(current) = current else { return };
    if incoming.is_some_and(|incoming| incoming == current || is_exact_negation(current, incoming)) && next.is_some() {
        *next = Some(current);
    }
}

fn field_is_protected(authority: UpdateAuthority, fence: Option<FieldStamp>) -> bool {
    authority == Untracked && fence.is_some_and(|fence| fence.authority == Tracked)
}

fn reject_untrusted_observation_groups(
    observation: &ObjectObservation,
    existing: Option<Entity>,
    world: &World,
    authority: UpdateAuthority,
    rejections: &mut Rejections,
) {
    if authority != Untracked {
        return;
    }
    let Some(object) = existing.and_then(|entity| world.object(entity)) else { return };
    let fences = object.data.fences;
    let has_pose = observation.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some());
    let has_motion = observation.linear_motion.is_some_and(|motion| motion.velocity.is_some() || motion.acceleration.is_some());
    if has_pose && field_is_protected(authority, fences.pose) {
        rejections.push(FieldRejection { group: FieldGroup::Pose, reason: UntrustedSource });
    }
    if has_motion && field_is_protected(authority, fences.linear_motion) {
        rejections.push(FieldRejection { group: FieldGroup::LinearMotion, reason: UntrustedSource });
    }
    if observation.angular_velocity.is_some() && field_is_protected(authority, fences.angular_velocity) {
        rejections.push(FieldRejection { group: FieldGroup::AngularVelocity, reason: UntrustedSource });
    }
    if observation.scale.is_some() && field_is_protected(authority, fences.scale) {
        rejections.push(FieldRejection { group: FieldGroup::Scale, reason: UntrustedSource });
    }
    if observation.attachment_point.is_some() && field_is_protected(authority, fences.attachment_point) {
        rejections.push(FieldRejection { group: FieldGroup::AttachmentPoint, reason: UntrustedSource });
    }
}

fn reject_untrusted_patch_groups(patch: &ObjectPatch, fences: Fences, authority: UpdateAuthority, rejections: &mut Rejections) {
    if authority != Untracked {
        return;
    }
    if patch.pose.is_some_and(|pose| pose.position.is_some() || pose.rotation.is_some()) && field_is_protected(authority, fences.pose) {
        rejections.push(FieldRejection { group: FieldGroup::Pose, reason: UntrustedSource });
    }
    if (patch.linear_motion.velocity.is_some() || patch.linear_motion.acceleration.is_some())
        && field_is_protected(authority, fences.linear_motion)
    {
        rejections.push(FieldRejection { group: FieldGroup::LinearMotion, reason: UntrustedSource });
    }
    if patch.angular_velocity.is_some() && field_is_protected(authority, fences.angular_velocity) {
        rejections.push(FieldRejection { group: FieldGroup::AngularVelocity, reason: UntrustedSource });
    }
    if patch.scale.is_some() && field_is_protected(authority, fences.scale) {
        rejections.push(FieldRejection { group: FieldGroup::Scale, reason: UntrustedSource });
    }
    if patch.attachment_point.is_some() && field_is_protected(authority, fences.attachment_point) {
        rejections.push(FieldRejection { group: FieldGroup::AttachmentPoint, reason: UntrustedSource });
    }
}

fn is_exact_negation(left: Quat, right: Quat) -> bool {
    left.x == -right.x && left.y == -right.y && left.z == -right.z && left.w == -right.w
}

fn apply_observation_values(
    data: &mut ObjectData,
    transform: &mut Transform,
    observation: &ObjectObservation,
    source: UpdateSource,
    stamp: Stamp,
) {
    let field_stamp = FieldStamp { authority: source.authority(), stamp };
    if let Some(pose) = observation.pose
        && (pose.position.is_some() || pose.rotation.is_some())
        && newer_field(field_stamp, data.fences.pose)
    {
        if let Some(position) = pose.position {
            transform.position = Some(position);
        }
        if let Some(rotation) = pose.rotation
            && !transform.rotation.is_some_and(|current| is_exact_negation(current, rotation))
        {
            transform.rotation = Some(rotation);
        }
        data.fences.pose = Some(field_stamp);
    }
    if let Some(motion) = observation.linear_motion
        && (motion.velocity.is_some() || motion.acceleration.is_some())
        && newer_field(field_stamp, data.fences.linear_motion)
    {
        patch_motion(&mut data.linear_motion, MotionPatch { velocity: motion.velocity, acceleration: motion.acceleration });
        data.fences.linear_motion = Some(field_stamp);
    }
    if let Some(value) = observation.angular_velocity
        && newer_field(field_stamp, data.fences.angular_velocity)
    {
        data.angular_velocity = Some(value);
        data.fences.angular_velocity = Some(field_stamp);
    }
    if let Some(value) = observation.scale
        && newer_field(field_stamp, data.fences.scale)
    {
        transform.scale = Some(value);
        data.fences.scale = Some(field_stamp);
    }
    if let Some(value) = observation.attachment_point
        && newer_field(field_stamp, data.fences.attachment_point)
    {
        data.attachment_point = value;
        data.fences.attachment_point = Some(field_stamp);
    }
}

fn patch_motion(current: &mut LinearMotion, patch: MotionPatch) -> bool {
    let before = *current;
    if let Some(value) = patch.velocity {
        current.velocity = Some(value);
    }
    if let Some(value) = patch.acceleration {
        current.acceleration = Some(value);
    }
    *current != before
}

fn recovery(mut targets: Vec<LocalAddress>, max: usize) -> Option<Recovery> {
    let mut seen = HashSet::new();
    targets.retain(|target| seen.insert(*target));
    targets.truncate(max);
    (!targets.is_empty()).then_some(Recovery { targets })
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use base::UUID;
    use math::{Quat, Vec3};

    use super::*;
    use crate::{
        AgentHandoff,
        ApplyError::RevisionExhausted,
        ApplyOutcome::{Deferred, RegionAdopted, TombstoneRecorded},
        DeferredReason::AmbiguousOrder,
        LinearMotion, ObjectObservation,
        OrderEvidence::Ordered,
        ParentUpdate::Root,
        Pose, WorldRevision,
    };

    fn test_world(regions: &[Region]) -> World {
        let tombstones = 8;
        let mut world = World::new(WorldLimits {
            max_region_registrations: regions.len(),
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: tombstones,
            max_tombstones: tombstones,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(tombstones),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(tombstones),
            max_journal_entries: 8,
            max_journal_changes: 32,
            max_region_name_bytes: 32,
            max_recovery_targets: 4,
        });
        for region in regions {
            assert_eq!(world.adopt_region(*region).unwrap().outcome, RegionAdopted);
        }
        world
    }

    #[test]
    fn invalid_patch_keeps_valid_groups() {
        let region = Region::new(23);
        let mut world = test_world(&[region]);
        let source = UpdateSource::new(region, 1, Ordered(1), 0);
        let observation = ObjectObservation {
            source,
            id: UUID::from_u128(23),
            local_id: NonZeroU32::new(1).unwrap(),
            kind: Kind::Object,
            parent: Root,
            pose: Some(Pose { position: Some(Vec3::X), rotation: None }),
            linear_motion: Some(LinearMotion::default()),
            angular_velocity: None,
            scale: Some(Vec3::splat(2.0)),
            attachment_point: None,
        };
        let introduced = world.observe_object(observation).unwrap();
        let ApplyOutcome::Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
        let revision = world.revision();

        let patch = ObjectPatch {
            pose: Some(Pose { position: Some(Vec3::Y), rotation: None }),
            scale: Some(Vec3::splat(f32::NAN)),
            ..ObjectPatch::new(UpdateSource::new(region, 2, Ordered(2), 0), NonZeroU32::new(1).unwrap())
        };

        let applied = world.patch_object(patch).unwrap();
        assert_eq!(applied.outcome, ApplyOutcome::Updated(entity));
        assert_eq!(applied.revision.unwrap().get(), revision.get() + 1);
        assert_eq!(
            applied.rejections.iter().copied().collect::<Vec<_>>(),
            [FieldRejection { group: FieldGroup::Scale, reason: FieldRejectionReason::InvalidValue { field: "scale" } }]
        );
        assert_eq!(world.revision().get(), revision.get() + 1);
        let object = world.object(entity).unwrap();
        assert_eq!(object.position(), Some(Vec3::Y));
        assert_eq!(object.scale(), Some(Vec3::splat(2.0)));
    }

    #[test]
    fn rotation_validation_enforces_tolerance() {
        let f32_overshoot = Quat::from_xyzw(1.0 + f32::EPSILON, 0.0, 0.0, 0.0);
        assert!(normalized_rotation(f32_overshoot).is_ok());
        // Adjacent representable magnitudes straddle the decoder tolerance bound.
        let edge = f32::from_bits(1.0_f32.to_bits() + 260);
        let outside_edge = f32::from_bits(1.0_f32.to_bits() + 261);
        assert!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, edge)).is_ok());
        assert!(matches!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, outside_edge)), Err(InvalidRotation)));
        assert!(matches!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)), Err(InvalidRotation)));
        assert!(matches!(normalized_rotation(Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0)), Err(InvalidRotation)));
        assert!(matches!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, 1.2)), Err(InvalidRotation)));
    }

    #[test]
    fn floor_defers_unknown_updates() {
        let region = Region::new(24);
        let mut world = test_world(&[region]);
        let address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        world.floors.insert(region, Stamp { order: 200, block_ordinal: 0 });
        let stale = UpdateSource::new(region, 150, Ordered(150), 0);

        let patch = world.patch_object(ObjectPatch::new(stale, address.local_id())).unwrap();
        assert_eq!(patch.outcome, Deferred(AmbiguousOrder));
        assert_eq!(patch.recovery.unwrap().targets, [address]);
        let removal = world.remove_object(stale, address.local_id()).unwrap();
        assert_eq!(removal.outcome, Deferred(AmbiguousOrder));
        assert_eq!(removal.recovery.unwrap().targets, [address]);
        assert_eq!(world.tombstones.get(&address), None);

        let newer = UpdateSource::new(region, 250, Ordered(250), 0);
        assert_eq!(world.remove_object(newer, address.local_id()).unwrap().outcome, TombstoneRecorded(address));
    }

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
            attachment_point: None,
        };
        let introduced = world.observe_object(observation).unwrap();
        let ApplyOutcome::Introduced(agent) = introduced.outcome else { panic!("expected agent introduction") };
        let source_address = LocalAddress::new(source_region, observation.local_id);
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
    fn tombstone_takes_precedence_over_floor() {
        let region = Region::new(20);
        let mut world = World::new(WorldLimits {
            max_region_registrations: 1,
            max_entities: 1,
            max_entities_per_region: 1,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: 4,
            max_journal_changes: 8,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        });
        assert_eq!(world.adopt_region(region).unwrap().outcome, ApplyOutcome::RegionAdopted);

        let address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        world.tombstones.insert(address, Stamp { order: 100, block_ordinal: 0 });
        world.floors.insert(region, Stamp { order: 200, block_ordinal: 0 });

        let result = world
            .observe_object(ObjectObservation {
                source: UpdateSource::new(region, 90, Ordered(90), 0),
                id: UUID::from_u128(20),
                local_id: NonZeroU32::new(1).unwrap(),
                kind: Kind::Object,
                parent: Root,
                pose: None,
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: None,
            })
            .unwrap();

        assert_eq!(result.outcome, ApplyOutcome::Unchanged);
        assert_eq!(result.recovery, None);
        assert_eq!(world.entity(UUID::from_u128(20)), None);
    }

    #[test]
    fn newer_than_tombstone_bypasses_floor() {
        let region = Region::new(22);
        let mut world = World::new(WorldLimits {
            max_region_registrations: 1,
            max_entities: 1,
            max_entities_per_region: 1,
            max_tombstones_per_region: 2,
            max_tombstones: 2,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(2),
            max_journal_entries: 4,
            max_journal_changes: 8,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        });
        assert_eq!(world.adopt_region(region).unwrap().outcome, ApplyOutcome::RegionAdopted);

        let killed_address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        let patched_address = LocalAddress::new(region, NonZeroU32::new(2).unwrap());
        let old = Stamp { order: 100, block_ordinal: 0 };
        world.tombstones.insert(killed_address, old);
        world.tombstones.insert(patched_address, old);
        world.floors.insert(region, Stamp { order: 200, block_ordinal: 0 });

        let killed = world.remove_object(UpdateSource::new(region, 150, Ordered(150), 0), killed_address.local_id()).unwrap();
        assert_eq!(killed.outcome, ApplyOutcome::TombstoneRecorded(killed_address));
        assert_eq!(world.tombstones.get(&killed_address), Some(&Stamp { order: 150, block_ordinal: 0 }));

        let patched =
            world.patch_object(ObjectPatch::new(UpdateSource::new(region, 150, Ordered(150), 0), patched_address.local_id())).unwrap();
        assert_eq!(patched.outcome, ApplyOutcome::Deferred(DeferredReason::UnknownObject));
        assert_eq!(patched.recovery.unwrap().targets, [patched_address]);
    }
}
