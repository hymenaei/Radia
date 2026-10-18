//! Field patches and targeted property updates.

use std::sync::Arc;

use base::UUID;

use super::super::journal::WorldChange::EntityUpdated;
use super::super::journal::WorldChangeClasses;
use super::super::{
    Object, ObjectContent, ObjectOrdering, ObjectRecord, ObjectRecordRef, ParentRejectionBarrier, RegionalStamp, World, newer_field,
    newer_region_field,
};
use crate::update::FieldStamp;
use crate::{
    Appearance, Applied, ApplyError, ApplyOutcome, AvatarAppearance, DeferredReason, FieldGroup, FieldRejection, FieldRejectionReason,
    Kind, LocalAddress, ObjectBindingTarget, ObjectMedia, ObjectPatch, Parent, PoseState, PropertiesPatch, PropertyProvenance, Recovery,
    Rejections, SitTarget, UpdateAuthority, UpdateSource,
};

use FieldRejectionReason::{ParentCycle, ParentDependsOnRejectedParent, UntrustedSource};
use UpdateAuthority::{Tracked, Untracked};

use super::fields::{
    apply_field_update, content_change_classes, field_at_or_after, has_pose, is_exact_negation, mark_pose_components, normalized_rotation,
    observable_change_classes, parent_frame_identity_changed, patch_motion, pose_is_accepted, pose_update_changes_observable,
    reject_untrusted_patch_groups, retain_stored_orientation, validate_patch, validate_uuid, validate_vec3,
};

const UNKNOWN_COF_VERSION: i32 = -1;

impl World {
    /// Applies source-ordered field groups to an existing simulator-local address.
    ///
    /// A patch never introduces an object. Missing identity or ambiguous ordering produces a
    /// deferred outcome and any available recovery targets. Invalid or protected groups are
    /// rejected independently, preserving eligible sibling changes.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] for invalid operation context or exhausted mutation capacity.
    /// The World remains unchanged on error.
    pub fn patch_object(&mut self, patch: ObjectPatch) -> Result<Applied, ApplyError> {
        let source = patch.source;
        let local_id = patch.local_id;
        self.require_active_region(source.region)?;
        let Some(stamp) = source.stamp() else {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![LocalAddress::new(source.region, local_id)], self.limits.max_recovery_targets),
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
            || patch.attachment_point.is_present()
            || patch.shape.is_present()
            || patch.appearance.is_present()
            || patch.surface_appearance.is_some()
            || patch.render_materials.is_some()
            || patch.gltf_material_overrides.is_some()
            || patch.avatar_appearance.is_some();
        let mut read = self.ecs.query_one::<(&ObjectRecord, &ObjectContent, &ObjectOrdering)>(entity.0);
        let (record, content, ordering) = read.get().expect("address index contains a complete entity");
        let object = Object { entity, record: ObjectRecordRef::Borrowed { record, content }, world: self };
        let data = &record.state;
        let transform = object.transform();
        let kind = object.kind();
        let object_parent = object.parent();
        let id = object.id();
        let incoming_rotation = patch.pose.and_then(|pose| pose.rotation);
        let current_rotation = transform.rotation();
        let (mut patch, mut rejections) = validate_patch(patch);
        if patch.object_media.is_some() && (kind != Kind::Object || object.content().appearance != Some(Appearance::Surface)) {
            rejections.push(FieldRejection {
                group: FieldGroup::ObjectMedia,
                reason: FieldRejectionReason::InvalidValue { field: "object_media" },
            });
            patch.object_media = None;
        }
        if patch.avatar_appearance.is_some() && kind != Kind::Avatar {
            rejections.push(FieldRejection {
                group: FieldGroup::AvatarAppearance,
                reason: FieldRejectionReason::InvalidValue { field: "avatar_appearance" },
            });
            patch.avatar_appearance = None;
        }
        if patch.gltf_material_overrides.is_some() && (kind != Kind::Object || object.content().appearance != Some(Appearance::Surface)) {
            rejections.push(FieldRejection {
                group: FieldGroup::GltfMaterialOverrides,
                reason: FieldRejectionReason::InvalidValue { field: "gltf_material_overrides" },
            });
            patch.gltf_material_overrides = None;
        }
        let stale_avatar_appearance = self.self_agent == Some(id)
            && patch.avatar_appearance.as_ref().is_some_and(|update| {
                let incoming = update.cof_version().unwrap_or(UNKNOWN_COF_VERSION);
                let current =
                    object.content().avatar_appearance.as_deref().and_then(AvatarAppearance::cof_version).unwrap_or(UNKNOWN_COF_VERSION);
                incoming <= current
            });
        if stale_avatar_appearance {
            rejections.push(FieldRejection { group: FieldGroup::AvatarAppearance, reason: FieldRejectionReason::StaleVersion });
            patch.avatar_appearance = None;
        }
        if let Some(pose) = patch.pose.as_mut() {
            retain_stored_orientation(current_rotation, incoming_rotation, &mut pose.rotation);
        }
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let parent_candidate = patch.parent.is_some()
            && newer_field(field_stamp, ordering.fences.parent)
            && ordering.parent_rejection_barrier.is_none_or(|barrier| barrier.admits(source, stamp));
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
            && (ordering.fences.parent.is_some_and(|fence| fence.authority == Tracked) || ordering.parent_rejection_barrier.is_some())
        {
            rejections.push(FieldRejection { group: FieldGroup::Parent, reason: UntrustedSource });
        }
        reject_untrusted_patch_groups(&patch, &ordering.fences, source.authority(), &mut rejections);
        let parent_is_newer = parent_candidate && !parent_cycle;
        let next_parent = if parent_is_newer { parent.expect("newer parent update was resolved") } else { data.parent };
        let motion_update_is_newer = (patch.linear_motion.velocity.is_some() || patch.linear_motion.acceleration.is_some())
            && newer_field(field_stamp, ordering.fences.linear_motion);
        let angular_velocity_update_is_newer =
            patch.angular_velocity.is_some() && newer_field(field_stamp, ordering.fences.angular_velocity);
        let scale_update_is_newer = patch.scale.is_some() && newer_field(field_stamp, ordering.fences.scale);
        let attachment_update_is_newer = patch.attachment_point.is_present() && newer_field(field_stamp, ordering.fences.attachment_point);
        let shape_update_is_newer = patch.shape.is_present() && newer_region_field(source.region, field_stamp, ordering.fences.shape);
        let appearance_update_is_newer =
            patch.appearance.is_present() && newer_region_field(source.region, field_stamp, ordering.fences.appearance);
        let texture_entry_update_is_newer = kind != Kind::Avatar
            && patch.surface_appearance.is_some()
            && newer_region_field(source.region, field_stamp, ordering.fences.texture_entry);
        let render_materials_update_is_newer = kind != Kind::Avatar
            && patch.render_materials.is_some()
            && newer_region_field(source.region, field_stamp, ordering.fences.render_materials);
        let gltf_material_overrides_update_is_newer = patch.gltf_material_overrides.is_some()
            && newer_region_field(source.region, field_stamp, ordering.fences.gltf_material_overrides);
        let avatar_appearance_update_is_newer =
            patch.avatar_appearance.is_some() && newer_region_field(source.region, field_stamp, ordering.fences.avatar_appearance);
        let mut next_avatar_appearance = avatar_appearance_update_is_newer.then(|| {
            let mut appearance = object.content().avatar_appearance.as_deref().cloned().unwrap_or_default();
            appearance.apply(patch.avatar_appearance.take().expect("newer avatar appearance update is present"));
            appearance
        });
        let mut object_media_update_is_newer = source.authority() == Tracked
            && patch
                .object_media
                .as_ref()
                .is_some_and(|update| object.content().object_media.as_deref().is_none_or(|current| update.version() > current.version()));
        let mut next_object_media = object_media_update_is_newer.then(|| {
            let mut media = object.content().object_media.as_deref().cloned().unwrap_or_default();
            media.apply(patch.object_media.as_ref().expect("newer ObjectMedia update is present"));
            media
        });
        if let Some(next) = next_object_media.as_ref() {
            let current_bytes = object.content().object_media.as_deref().map_or(0, ObjectMedia::retained_bytes);
            let next_total = self.object_media_bytes.saturating_sub(current_bytes).saturating_add(next.retained_bytes());
            if next_total > super::super::MAX_OBJECT_MEDIA_BYTES {
                rejections.push(FieldRejection {
                    group: FieldGroup::ObjectMedia,
                    reason: FieldRejectionReason::ResourceLimit { field: "object_media" },
                });
                patch.object_media = None;
                next_object_media = None;
                object_media_update_is_newer = false;
            }
        }
        let next_attachment_point = if attachment_update_is_newer {
            apply_field_update(data.attachment_point, patch.attachment_point)
        } else {
            data.attachment_point
        };
        let attachment_frame = |parent: Parent| matches!(parent, Parent::Resolved(parent) if self.record(parent).is_some_and(|record| record.kind == Kind::Avatar));
        let frame_changed = (parent_is_newer && parent_frame_identity_changed(self, data.parent, next_parent))
            || (attachment_update_is_newer
                && next_attachment_point != data.attachment_point
                && (attachment_frame(data.parent) || attachment_frame(next_parent)));
        let next_frame_fence = if frame_changed { Some(field_stamp) } else { ordering.fences.frame };
        let next_frame_generation = if frame_changed {
            data.frame_generation.checked_add(1).ok_or(ApplyError::FrameGenerationExhausted)?
        } else {
            data.frame_generation
        };
        let pose_is_accepted = pose_is_accepted(patch.pose, field_stamp, Some(&ordering.fences), next_frame_fence);
        if patch.pose.is_some_and(has_pose)
            && newer_field(field_stamp, ordering.fences.pose)
            && !field_at_or_after(field_stamp, next_frame_fence)
        {
            rejections.push(FieldRejection { group: FieldGroup::Pose, reason: FieldRejectionReason::StaleFrame });
        }
        if !pose_is_accepted
            && !parent_is_newer
            && !motion_update_is_newer
            && !angular_velocity_update_is_newer
            && !scale_update_is_newer
            && !attachment_update_is_newer
            && !shape_update_is_newer
            && !appearance_update_is_newer
            && !texture_entry_update_is_newer
            && !render_materials_update_is_newer
            && !gltf_material_overrides_update_is_newer
            && !avatar_appearance_update_is_newer
            && !object_media_update_is_newer
            && !(parent_cycle && source.authority() == Tracked)
            && !(has_update && source.authority() == Tracked && stamp > data.binding.fence)
        {
            return Ok(Applied::unchanged().with_rejections(rejections));
        }
        let pose_affects_descendants = pose_update_changes_observable(data, ordering, patch.pose, field_stamp, next_frame_fence);
        let detach_snapshot = if parent_is_newer
            && next_parent == Parent::Root
            && kind == Kind::Avatar
            && matches!(data.parent, Parent::Resolved(parent) if self.record(parent).is_some_and(|record| record.kind != Kind::Avatar))
        {
            match self.resolve_pose(entity).ok() {
                Some(PoseState::Resolved(pose)) => Some(pose),
                Some(PoseState::Unresolved(_) | PoseState::Provisional { .. }) | None => None,
            }
        } else {
            None
        };
        let parent_changed = parent_is_newer && object_parent != next_parent;
        let mut object_media_size_delta = None;
        let mut data = *data;
        let mut content = (shape_update_is_newer
            || appearance_update_is_newer
            || texture_entry_update_is_newer
            || render_materials_update_is_newer
            || gltf_material_overrides_update_is_newer
            || avatar_appearance_update_is_newer
            || object_media_update_is_newer)
            .then(|| object.content().clone());
        let mut ordering = *ordering;
        data.frame_generation = next_frame_generation;
        if frame_changed {
            data.provisional_pose = None;
            ordering.fences.frame = next_frame_fence;
        }
        if let Some(pose) = patch.pose.filter(|_| pose_is_accepted) {
            if let Some(position) = pose.position {
                data.transform.position = Some(position);
            }
            if let Some(rotation) = pose.rotation
                && !data.transform.rotation.is_some_and(|current| is_exact_negation(current, rotation))
            {
                data.transform.rotation = Some(rotation);
            }
            ordering.fences.pose = Some(field_stamp);
        }
        if motion_update_is_newer {
            patch_motion(&mut data.linear_motion, patch.linear_motion);
            ordering.fences.linear_motion = Some(field_stamp);
        }
        if let Some(value) = patch.angular_velocity.filter(|_| angular_velocity_update_is_newer) {
            data.angular_velocity = Some(value);
            ordering.fences.angular_velocity = Some(field_stamp);
        }
        if let Some(value) = patch.scale.filter(|_| scale_update_is_newer) {
            data.transform.scale = Some(value);
            ordering.fences.scale = Some(field_stamp);
        }
        if attachment_update_is_newer {
            data.attachment_point = next_attachment_point;
            ordering.fences.attachment_point = Some(field_stamp);
        }
        if let Some(content) = content.as_mut() {
            if shape_update_is_newer {
                content.shape = apply_field_update(content.shape, patch.shape);
                ordering.fences.shape = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if appearance_update_is_newer {
                content.appearance = apply_field_update(content.appearance, patch.appearance);
                ordering.fences.appearance = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if texture_entry_update_is_newer {
                let value = patch.surface_appearance.take().expect("newer TextureEntry update is present");
                if content.surface_appearance.as_deref() != Some(&value) {
                    content.surface_appearance = Some(Arc::new(value));
                }
                ordering.fences.texture_entry = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if render_materials_update_is_newer {
                let value = patch.render_materials.take().expect("newer RenderMaterials update is present");
                if content.render_materials.as_deref() != (!value.is_empty()).then_some(&value) {
                    content.render_materials = (!value.is_empty()).then(|| Arc::new(value));
                }
                ordering.fences.render_materials = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if gltf_material_overrides_update_is_newer {
                let value = patch.gltf_material_overrides.take().expect("newer GLTF material override update is present");
                if content.gltf_material_overrides.as_deref() != (!value.is_empty()).then_some(&value) {
                    content.gltf_material_overrides = (!value.is_empty()).then(|| Arc::new(value));
                }
                ordering.fences.gltf_material_overrides = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if avatar_appearance_update_is_newer {
                let value = next_avatar_appearance.take().expect("newer avatar appearance was merged");
                content.avatar_appearance = Some(Arc::new(value));
                ordering.fences.avatar_appearance = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            }
            if object_media_update_is_newer {
                let value = next_object_media.take().expect("newer ObjectMedia update has been applied");
                object_media_size_delta =
                    Some((content.object_media.as_deref().map_or(0, ObjectMedia::retained_bytes), value.retained_bytes()));
                content.object_media = Some(Arc::new(value));
            }
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
            ordering.fences.parent = Some(field_stamp);
            ordering.parent_rejection_barrier = None;
        } else if parent_cycle && source.authority() == Tracked {
            ordering.parent_rejection_barrier = Some(ParentRejectionBarrier { region: source.region, stamp });
        }
        if has_update && source.authority() == Tracked {
            data.binding.fence = data.binding.fence.max(stamp);
        }
        let change_classes = observable_change_classes(&object.record.state, object_parent, &data, next_parent);
        let change_classes =
            content.as_ref().map_or(change_classes, |content| change_classes | content_change_classes(object.content(), content));
        drop(object);
        drop(read);
        let observable_change = !change_classes.is_empty();
        if observable_change {
            self.ensure_revision_available()?;
        }
        self.commit_values(entity, data, content, ordering);
        if let Some((old_bytes, new_bytes)) = object_media_size_delta {
            self.object_media_bytes = self.object_media_bytes.saturating_sub(old_bytes).saturating_add(new_bytes);
        }
        let mut changes = Vec::new();
        let mut recovery_targets = Vec::new();
        if let Some(parent) = parent.filter(|_| parent_is_newer) {
            self.apply_parent(entity, parent, &mut recovery_targets);
        }
        if observable_change {
            changes.push(EntityUpdated { id, entity, classes: change_classes });
        }
        if frame_changed || parent_changed || pose_affects_descendants {
            self.invalidate_subtree(entity, &mut changes);
        }
        Ok(self
            .finish_mutation(
                if observable_change { ApplyOutcome::Updated(entity) } else { ApplyOutcome::Unchanged },
                changes,
                Recovery::from_targets(recovery_targets, self.limits.max_recovery_targets),
            )
            .with_rejections(rejections)
            .with_recovery_satisfied(parent_is_newer))
    }

    /// Applies an asynchronous property response to its captured object binding.
    ///
    /// `response_id` must match the request target. A response for an obsolete binding is ignored,
    /// so address reuse or transfer cannot redirect it to another object. Owner and creator
    /// evidence are accepted independently and retain their source provenance.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] for invalid context, identity values, or exhausted revision
    /// capacity. The World remains unchanged on error.
    pub fn patch_properties(
        &mut self,
        source: UpdateSource,
        target: ObjectBindingTarget,
        response_id: UUID,
        patch: PropertiesPatch,
    ) -> Result<Applied, ApplyError> {
        self.require_active_region(source.region)?;
        validate_uuid(response_id, "object_id")?;
        if response_id != target.id || target.address.region != source.region || !self.object_binding_target_is_current(target) {
            return Ok(self.finish_mutation(ApplyOutcome::IgnoredStale, Vec::new(), Recovery::default()));
        }
        let Some(stamp) = source.stamp() else {
            return Ok(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![target.address], self.limits.max_recovery_targets),
            ));
        };
        let entity = target.entity;
        let has_property_update = patch.owner_id.is_some() || patch.creator_id.is_some();
        let (properties, provenance) = {
            let object = self.object(entity).expect("current properties target names a live entity");
            (object.content().properties, object.content().properties_provenance)
        };
        let owner_fence = provenance.owner.map(RegionalStamp::from);
        let creator_fence = provenance.creator.map(RegionalStamp::from);
        let field_stamp = FieldStamp { authority: source.authority(), stamp };
        let mut rejections = Rejections::default();
        let owner_untrusted = patch.owner_id.is_some()
            && source.authority() == Untracked
            && owner_fence.is_some_and(|fence| fence.stamp.authority == Tracked);
        let creator_untrusted = patch.creator_id.is_some()
            && source.authority() == Untracked
            && creator_fence.is_some_and(|fence| fence.stamp.authority == Tracked);
        if owner_untrusted || creator_untrusted {
            rejections.push(FieldRejection { group: FieldGroup::Properties, reason: UntrustedSource });
        }
        let owner_is_newer = patch.owner_id.is_some() && !owner_untrusted && newer_region_field(source.region, field_stamp, owner_fence);
        let creator_is_newer =
            patch.creator_id.is_some() && !creator_untrusted && newer_region_field(source.region, field_stamp, creator_fence);
        let next_provenance = PropertyProvenance {
            region: source.region,
            authority: source.authority(),
            order: stamp.order,
            block_ordinal: stamp.block_ordinal,
            source_sequence: source.sequence(),
        };
        let observable_change = (owner_is_newer && (properties.owner_id != patch.owner_id || provenance.owner != Some(next_provenance)))
            || (creator_is_newer && (properties.creator_id != patch.creator_id || provenance.creator != Some(next_provenance)));
        if observable_change {
            self.ensure_revision_available()?;
        }
        {
            let (record, content) =
                self.ecs.query_one_mut::<(&mut ObjectRecord, &mut ObjectContent)>(entity.0).expect("property target has state and content");
            if owner_is_newer && let Some(value) = patch.owner_id {
                content.properties.owner_id = Some(value);
                content.properties_provenance.owner = Some(next_provenance);
            }
            if creator_is_newer && let Some(value) = patch.creator_id {
                content.properties.creator_id = Some(value);
                content.properties_provenance.creator = Some(next_provenance);
            }
            if has_property_update && source.authority() == Tracked {
                record.state.binding.fence = record.state.binding.fence.max(stamp);
            }
        }
        let changes = if observable_change {
            vec![EntityUpdated { id: response_id, entity, classes: WorldChangeClasses::PROPERTIES }]
        } else {
            Vec::new()
        };
        Ok(self
            .finish_mutation(
                if observable_change { ApplyOutcome::Updated(entity) } else { ApplyOutcome::Unchanged },
                changes,
                Recovery::default(),
            )
            .with_rejections(rejections))
    }

    /// Applies or clears simulator sit-target evidence for an object UUID in the source region.
    ///
    /// `None` clears the target. A missing object or ambiguous source order produces a deferred
    /// outcome. Invalid target values are reported as a rejected field group.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] for invalid operation context or exhausted revision capacity.
    /// The World remains unchanged on error.
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
                recovery_address
                    .map_or_else(Recovery::default, |address| Recovery::from_targets(vec![address], self.limits.max_recovery_targets)),
            ));
        };
        let Some(entity) = self.entities.get(&id).copied() else {
            return Ok(Applied::deferred(DeferredReason::UnknownObject, Recovery::default()));
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

        let (sit_target, sit_target_fence) = (object.content().sit_target, object.ordering().fences.sit_target);
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
            let (record, content, ordering) = self
                .ecs
                .query_one_mut::<(&mut ObjectRecord, &mut ObjectContent, &mut ObjectOrdering)>(entity.0)
                .expect("sit target has all components");
            content.sit_target = target;
            ordering.fences.sit_target = Some(RegionalStamp { region: source.region, stamp: field_stamp });
            if source.authority() == Tracked {
                record.state.binding.fence = record.state.binding.fence.max(stamp);
            }
        }
        let changes =
            if observable_change { vec![EntityUpdated { id, entity, classes: WorldChangeClasses::RELATIONSHIP }] } else { Vec::new() };
        Ok(self
            .finish_mutation(
                if observable_change { ApplyOutcome::Updated(entity) } else { ApplyOutcome::Unchanged },
                changes,
                Recovery::default(),
            )
            .with_rejections(rejections))
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use base::UUID;
    use math::Vec3;

    use crate::{
        Appearance, ApplyOutcome, ApplyOutcome::Introduced, FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate, Kind,
        LinearMotion, MediaEntry, ObjectMediaUpdate, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate::Root, Pose,
        Region, UpdateSource,
    };

    use super::super::tests::test_world;
    use crate::world::MAX_OBJECT_MEDIA_BYTES;

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
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Keep,
            surface_appearance: None,
            render_materials: None,
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
    fn object_media_respects_retained_budget() {
        let region = Region::new(24);
        let mut world = test_world(&[region]);
        let local_id = NonZeroU32::new(1).unwrap();
        let object = ObjectObservation {
            source: UpdateSource::new(region, 1, Ordered(1), 0),
            id: UUID::from_u128(24),
            local_id,
            kind: Kind::Object,
            parent: Root,
            pose: None,
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: FieldUpdate::Keep,
            shape: FieldUpdate::Keep,
            appearance: FieldUpdate::Set(Appearance::Surface),
            surface_appearance: None,
            render_materials: None,
        };
        let introduced = world.observe_object(object).unwrap();
        let Introduced(entity) = introduced.outcome else { panic!("expected object introduction") };
        world.object_media_bytes = MAX_OBJECT_MEDIA_BYTES;

        let update = ObjectMediaUpdate::new(1, vec![Some(MediaEntry::default())]).unwrap();
        let result = world
            .patch_object(ObjectPatch {
                object_media: Some(update),
                ..ObjectPatch::new(UpdateSource::new(region, 3, Ordered(3), 0), local_id)
            })
            .unwrap();

        assert!(result.rejections.iter().any(|rejection| *rejection
            == FieldRejection { group: FieldGroup::ObjectMedia, reason: FieldRejectionReason::ResourceLimit { field: "object_media" } }));
        assert_eq!(world.object(entity).unwrap().object_media(), None);
        assert_eq!(world.object_media_bytes, MAX_OBJECT_MEDIA_BYTES);
    }
}
