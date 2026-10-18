//! Validation, source acceptance, and value rules for object field groups.

use std::sync::Arc;

use base::UUID;
use math::{Quat, Vec3};

use super::super::journal::WorldChangeClasses;
use super::super::{Fences, ObjectContent, ObjectOrdering, ObjectState, RegionalStamp, newer_field, newer_region_field};
use crate::update::{FieldStamp, Stamp};
use crate::{
    ApplyError, FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate, Kind, LinearMotion, MotionPatch, ObjectObservation,
    ObjectPatch, Parent, Pose, Rejections, RenderMaterials, SurfaceAppearance, UpdateAuthority, UpdateSource,
};

use FieldRejectionReason::{InvalidRotation, UntrustedSource};
use UpdateAuthority::{Tracked, Untracked};

pub(super) fn observable_change_classes(
    before: &ObjectState,
    before_parent: Parent,
    after: &ObjectState,
    after_parent: Parent,
) -> WorldChangeClasses {
    let mut classes = WorldChangeClasses::default();

    if before.transform.position() != after.transform.position()
        || before.transform.rotation() != after.transform.rotation()
        || before.frame_generation != after.frame_generation
        || before.position_frame_generation != after.position_frame_generation
        || before.rotation_frame_generation != after.rotation_frame_generation
        || before.provisional_pose != after.provisional_pose
        || before_parent != after_parent
    {
        classes |= WorldChangeClasses::POSE;
    }

    if before.linear_motion != after.linear_motion || before.angular_velocity != after.angular_velocity {
        classes |= WorldChangeClasses::MOTION;
    }
    if before.transform.scale() != after.transform.scale() {
        classes |= WorldChangeClasses::SCALE;
    }
    if before_parent != after_parent || before.attachment_point != after.attachment_point {
        classes |= WorldChangeClasses::RELATIONSHIP;
    }

    classes
}

pub(super) fn content_change_classes(before_content: &ObjectContent, after_content: &ObjectContent) -> WorldChangeClasses {
    let mut classes = WorldChangeClasses::default();
    if before_content.sit_target != after_content.sit_target {
        classes |= WorldChangeClasses::RELATIONSHIP;
    }
    if before_content.shape != after_content.shape {
        classes |= WorldChangeClasses::SHAPE;
    }

    if before_content.appearance != after_content.appearance
        || !same_arc(before_content.surface_appearance.as_ref(), after_content.surface_appearance.as_ref())
        || !same_arc(before_content.render_materials.as_ref(), after_content.render_materials.as_ref())
        || !same_arc(before_content.gltf_material_overrides.as_ref(), after_content.gltf_material_overrides.as_ref())
        || !same_arc(before_content.avatar_appearance.as_ref(), after_content.avatar_appearance.as_ref())
    {
        classes |= WorldChangeClasses::APPEARANCE;
    }

    if !same_arc(before_content.object_media.as_ref(), after_content.object_media.as_ref()) {
        classes |= WorldChangeClasses::MEDIA;
    }
    if before_content.properties != after_content.properties || before_content.properties_provenance != after_content.properties_provenance
    {
        classes |= WorldChangeClasses::PROPERTIES;
    }

    classes
}

fn same_arc<T: PartialEq>(before: Option<&Arc<T>>, after: Option<&Arc<T>>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(before), Some(after)) => Arc::ptr_eq(before, after) || **before == **after,
        _ => false,
    }
}

pub(super) fn validate_uuid(id: UUID, field: &'static str) -> Result<(), ApplyError> {
    if id.is_nil() { Err(ApplyError::InvalidValue { field }) } else { Ok(()) }
}

fn pose_frame_markers_change(data: &ObjectState, pose: Pose) -> bool {
    (pose.position.is_some() && data.position_frame_generation != Some(data.frame_generation))
        || (pose.rotation.is_some() && data.rotation_frame_generation != Some(data.frame_generation))
}

pub(super) fn has_pose(pose: Pose) -> bool {
    pose.position.is_some() || pose.rotation.is_some()
}

pub(super) fn pose_is_accepted(pose: Option<Pose>, stamp: FieldStamp, fences: Option<&Fences>, frame_fence: Option<FieldStamp>) -> bool {
    pose.is_some_and(has_pose) && fences.is_none_or(|fences| newer_field(stamp, fences.pose)) && field_at_or_after(stamp, frame_fence)
}

pub(super) fn field_at_or_after(stamp: FieldStamp, fence: Option<FieldStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.authority) {
        (Tracked, Untracked) => true,
        (Untracked, Tracked) => false,
        _ => stamp.stamp >= fence.stamp,
    })
}

pub(super) fn pose_update_changes_observable(
    data: &ObjectState,
    ordering: &ObjectOrdering,
    pose: Option<Pose>,
    stamp: FieldStamp,
    frame_fence: Option<FieldStamp>,
) -> bool {
    let Some(pose) = pose.filter(|pose| has_pose(*pose)) else { return false };
    if !newer_field(stamp, ordering.fences.pose) || !field_at_or_after(stamp, frame_fence) {
        return false;
    }
    data.provisional_pose.is_some()
        || pose_frame_markers_change(data, pose)
        || pose.position.is_some_and(|position| data.transform.position() != Some(position))
        || pose.rotation.is_some_and(|rotation| rotation_changes(data.transform.rotation(), rotation))
}

pub(super) fn mark_pose_components(data: &mut ObjectState, pose: Pose) {
    if pose.position.is_some() {
        data.position_frame_generation = Some(data.frame_generation);
    }
    if pose.rotation.is_some() {
        data.rotation_frame_generation = Some(data.frame_generation);
    }
}

pub(super) fn validate_vec3(value: Option<Vec3>, field: &'static str) -> Result<(), FieldRejectionReason> {
    if value.is_some_and(|value| !value.is_finite()) { Err(FieldRejectionReason::InvalidValue { field }) } else { Ok(()) }
}

pub(super) fn normalized_rotation(value: Quat) -> Result<Quat, FieldRejectionReason> {
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

fn validate_pose(value: &mut Option<Pose>, rejections: &mut Rejections) {
    if let Some(mut pose) = *value
        && (pose.position.is_some() || pose.rotation.is_some())
    {
        let invalid_position = validate_vec3(pose.position, "position").err();
        let rotation = pose.rotation.map(normalized_rotation).transpose();
        let invalid_rotation = rotation.as_ref().err().copied();
        if let Some(reason) = invalid_position.or(invalid_rotation) {
            rejections.push(FieldRejection { group: FieldGroup::Pose, reason });
            *value = None;
        } else {
            pose.rotation = rotation.expect("rotation was validated");
            *value = Some(pose);
        }
    }
}

fn validate_linear_motion(velocity: Option<Vec3>, acceleration: Option<Vec3>) -> Result<(), FieldRejectionReason> {
    validate_vec3(velocity, "velocity")?;
    validate_vec3(acceleration, "acceleration")
}

pub(super) fn validate_observation(mut observation: ObjectObservation) -> (ObjectObservation, Rejections) {
    let mut rejections = Rejections::default();
    validate_pose(&mut observation.pose, &mut rejections);
    if let Some(motion) = observation.linear_motion
        && (motion.velocity.is_some() || motion.acceleration.is_some())
    {
        let reason = validate_linear_motion(motion.velocity, motion.acceleration).err();
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

pub(super) fn validate_patch(mut patch: ObjectPatch) -> (ObjectPatch, Rejections) {
    let mut rejections = Rejections::default();
    validate_pose(&mut patch.pose, &mut rejections);
    if let Some(reason) = validate_linear_motion(patch.linear_motion.velocity, patch.linear_motion.acceleration).err() {
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
    if patch.object_media.as_ref().is_some_and(|update| update.validate().is_err()) {
        rejections
            .push(FieldRejection { group: FieldGroup::ObjectMedia, reason: FieldRejectionReason::InvalidValue { field: "object_media" } });
        patch.object_media = None;
    }
    (patch, rejections)
}

fn rotation_changes(current: Option<Quat>, next: Quat) -> bool {
    current.is_none_or(|current| current != next && !is_exact_negation(current, next))
}

pub(super) fn retain_stored_orientation(current: Option<Quat>, incoming: Option<Quat>, next: &mut Option<Quat>) {
    let Some(current) = current else { return };
    if incoming.is_some_and(|incoming| incoming == current || is_exact_negation(current, incoming)) && next.is_some() {
        *next = Some(current);
    }
}

pub(super) fn field_is_protected(authority: UpdateAuthority, fence: Option<FieldStamp>) -> bool {
    authority == Untracked && fence.is_some_and(|fence| fence.authority == Tracked)
}

pub(super) fn reject_untrusted_observation_groups(
    observation: &ObjectObservation,
    fences: Option<&Fences>,
    authority: UpdateAuthority,
    rejections: &mut Rejections,
) {
    if authority != Untracked {
        return;
    }
    let Some(fences) = fences else { return };
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
    if observation.attachment_point.is_present() && field_is_protected(authority, fences.attachment_point) {
        rejections.push(FieldRejection { group: FieldGroup::AttachmentPoint, reason: UntrustedSource });
    }
    if observation.shape.is_present() && field_is_protected(authority, fences.shape.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::Shape, reason: UntrustedSource });
    }
    if observation.appearance.is_present() && field_is_protected(authority, fences.appearance.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::Appearance, reason: UntrustedSource });
    }
    if observation.surface_appearance.is_some() && field_is_protected(authority, fences.texture_entry.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::TextureEntry, reason: UntrustedSource });
    }
    if observation.render_materials.is_some() && field_is_protected(authority, fences.render_materials.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::RenderMaterials, reason: UntrustedSource });
    }
}

pub(super) fn reject_untrusted_patch_groups(patch: &ObjectPatch, fences: &Fences, authority: UpdateAuthority, rejections: &mut Rejections) {
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
    if patch.attachment_point.is_present() && field_is_protected(authority, fences.attachment_point) {
        rejections.push(FieldRejection { group: FieldGroup::AttachmentPoint, reason: UntrustedSource });
    }
    if patch.shape.is_present() && field_is_protected(authority, fences.shape.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::Shape, reason: UntrustedSource });
    }
    if patch.appearance.is_present() && field_is_protected(authority, fences.appearance.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::Appearance, reason: UntrustedSource });
    }
    if patch.surface_appearance.is_some() && field_is_protected(authority, fences.texture_entry.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::TextureEntry, reason: UntrustedSource });
    }
    if patch.render_materials.is_some() && field_is_protected(authority, fences.render_materials.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::RenderMaterials, reason: UntrustedSource });
    }
    if patch.gltf_material_overrides.is_some() && field_is_protected(authority, fences.gltf_material_overrides.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::GltfMaterialOverrides, reason: UntrustedSource });
    }
    if patch.avatar_appearance.is_some() && field_is_protected(authority, fences.avatar_appearance.map(|fence| fence.stamp)) {
        rejections.push(FieldRejection { group: FieldGroup::AvatarAppearance, reason: UntrustedSource });
    }
    if patch.object_media.is_some() && authority == Untracked {
        rejections.push(FieldRejection { group: FieldGroup::ObjectMedia, reason: UntrustedSource });
    }
}

pub(super) fn is_exact_negation(left: Quat, right: Quat) -> bool {
    left.x == -right.x && left.y == -right.y && left.z == -right.z && left.w == -right.w
}

pub(super) fn apply_observation_values(
    data: &mut ObjectState,
    content: &mut ObjectContent,
    ordering: &mut ObjectOrdering,
    observation: &ObjectObservation,
    surface_appearance: Option<SurfaceAppearance>,
    render_materials: Option<RenderMaterials>,
    source: UpdateSource,
    stamp: Stamp,
    pose_is_accepted: bool,
) {
    let field_stamp = FieldStamp { authority: source.authority(), stamp };
    if let Some(pose) = observation.pose.filter(|_| pose_is_accepted)
        && (pose.position.is_some() || pose.rotation.is_some())
        && newer_field(field_stamp, ordering.fences.pose)
    {
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
    if let Some(motion) = observation.linear_motion
        && (motion.velocity.is_some() || motion.acceleration.is_some())
        && newer_field(field_stamp, ordering.fences.linear_motion)
    {
        patch_motion(&mut data.linear_motion, MotionPatch { velocity: motion.velocity, acceleration: motion.acceleration });
        ordering.fences.linear_motion = Some(field_stamp);
    }
    if let Some(value) = observation.angular_velocity
        && newer_field(field_stamp, ordering.fences.angular_velocity)
    {
        data.angular_velocity = Some(value);
        ordering.fences.angular_velocity = Some(field_stamp);
    }
    if let Some(value) = observation.scale
        && newer_field(field_stamp, ordering.fences.scale)
    {
        data.transform.scale = Some(value);
        ordering.fences.scale = Some(field_stamp);
    }
    if observation.attachment_point.is_present() && newer_field(field_stamp, ordering.fences.attachment_point) {
        data.attachment_point = apply_field_update(data.attachment_point, observation.attachment_point);
        ordering.fences.attachment_point = Some(field_stamp);
    }
    if observation.shape.is_present() && newer_region_field(source.region, field_stamp, ordering.fences.shape) {
        content.shape = apply_field_update(content.shape, observation.shape);
        ordering.fences.shape = Some(RegionalStamp { region: source.region, stamp: field_stamp });
    }
    if observation.appearance.is_present() && newer_region_field(source.region, field_stamp, ordering.fences.appearance) {
        content.appearance = apply_field_update(content.appearance, observation.appearance);
        ordering.fences.appearance = Some(RegionalStamp { region: source.region, stamp: field_stamp });
    }
    if let Some(value) = surface_appearance
        && observation.kind != Kind::Avatar
        && newer_region_field(source.region, field_stamp, ordering.fences.texture_entry)
    {
        if content.surface_appearance.as_deref() != Some(&value) {
            content.surface_appearance = Some(Arc::new(value));
        }
        ordering.fences.texture_entry = Some(RegionalStamp { region: source.region, stamp: field_stamp });
    }
    if let Some(value) = render_materials
        && observation.kind != Kind::Avatar
        && newer_region_field(source.region, field_stamp, ordering.fences.render_materials)
    {
        if content.render_materials.as_deref() != (!value.is_empty()).then_some(&value) {
            content.render_materials = (!value.is_empty()).then(|| Arc::new(value));
        }
        ordering.fences.render_materials = Some(RegionalStamp { region: source.region, stamp: field_stamp });
    }
}

pub(super) fn apply_field_update<T: Copy>(current: Option<T>, update: FieldUpdate<T>) -> Option<T> {
    match update {
        FieldUpdate::Keep => current,
        FieldUpdate::Set(value) => Some(value),
        FieldUpdate::Clear => None,
    }
}

pub(super) fn patch_motion(current: &mut LinearMotion, patch: MotionPatch) {
    if let Some(value) = patch.velocity {
        current.velocity = Some(value);
    }
    if let Some(value) = patch.acceleration {
        current.acceleration = Some(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_validation_enforces_tolerance() {
        let f32_overshoot = Quat::from_xyzw(1.0 + f32::EPSILON, 0.0, 0.0, 0.0);
        assert!(normalized_rotation(f32_overshoot).is_ok());
        let edge = f32::from_bits(1.0_f32.to_bits() + 260);
        let outside_edge = f32::from_bits(1.0_f32.to_bits() + 261);
        assert!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, edge)).is_ok());
        assert!(matches!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, outside_edge)), Err(InvalidRotation)));
        assert!(matches!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)), Err(InvalidRotation)));
        assert!(matches!(normalized_rotation(Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0)), Err(InvalidRotation)));
        assert!(matches!(normalized_rotation(Quat::from_xyzw(0.0, 0.0, 0.0, 1.2)), Err(InvalidRotation)));
    }
}
