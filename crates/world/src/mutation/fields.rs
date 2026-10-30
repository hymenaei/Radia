use math::{Quat, Vec3};
use uuid::Uuid;

use super::facet::{FacetChanges, FacetSet, FacetUpdates};
use crate::FieldRejectionReason::{InvalidRotation, InvalidValue, ParentCycle, ParentDependsOnRejectedParent, StaleFrame, UntrustedSource};
use crate::UpdateAuthority::{Tracked, Untracked};
use crate::history::WorldChangeClasses;
use crate::update::FieldStamp;
use crate::{
    Appearance, ApplyError, AttachmentPoint, FieldGroup, FieldRejectionReason, FieldUpdate, Kind, LinearMotion, MotionUpdateKind,
    ObjectObservation, ObjectPatch, Parent, ParentUpdate, Pose, Rejections, UpdateAuthority, WorldPose,
};
use crate::{Fences, ObjectOrdering, ObjectState, ParentRejectionBarrier};

pub(super) use super::facet::Origin;

const FULL_MOTION_COMPONENT_LIMIT: f32 = 256.0;
const TERSE_VELOCITY_COMPONENT_LIMIT: f32 = 128.0;
const TERSE_ACCELERATION_COMPONENT_LIMIT: f32 = 64.0;
const ANGULAR_VELOCITY_COMPONENT_LIMIT: f32 = 64.0;

pub(super) struct Fields {
    pub(super) pose: Option<Pose>,
    linear_motion: LinearMotion,
    motion_update_kind: MotionUpdateKind,
    angular_velocity: Option<Vec3>,
    is_physical: Option<bool>,
    time_dilation: Option<f32>,
    scale: Option<Vec3>,
    pub(super) attachment_point: FieldUpdate<AttachmentPoint>,
    pub(super) facets: FacetUpdates,
}

fn present_update<T>(update: FieldUpdate<T>) -> Option<FieldUpdate<T>> {
    update.is_present().then_some(update)
}

impl Fields {
    pub(super) fn observed(observation: ObjectObservation) -> Self {
        Self {
            pose: observation.pose,
            linear_motion: observation.linear_motion.unwrap_or_default(),
            motion_update_kind: observation.source.motion_update_kind(),
            angular_velocity: observation.angular_velocity,
            is_physical: observation.is_physical,
            time_dilation: observation.time_dilation,
            scale: observation.scale,
            attachment_point: observation.attachment_point,
            facets: FacetUpdates {
                shape: present_update(observation.shape),
                appearance: present_update(observation.appearance),
                surface_appearance: observation.surface_appearance,
                texture_animation: present_update(observation.texture_animation),
                pbr_materials: observation.pbr_materials,
                ..FacetUpdates::default()
            },
        }
    }

    pub(super) fn patched(patch: ObjectPatch) -> (Option<ParentUpdate>, Self) {
        let fields = Self {
            pose: patch.pose,
            linear_motion: patch.linear_motion,
            motion_update_kind: patch.source.motion_update_kind(),
            angular_velocity: patch.angular_velocity,
            is_physical: patch.is_physical,
            time_dilation: patch.time_dilation,
            scale: patch.scale,
            attachment_point: patch.attachment_point,
            facets: FacetUpdates {
                shape: present_update(patch.shape),
                appearance: present_update(patch.appearance),
                surface_appearance: patch.surface_appearance,
                texture_animation: present_update(patch.texture_animation),
                pbr_materials: patch.pbr_materials,
                pbr_material_overrides: patch.pbr_material_overrides,
                avatar_appearance: patch.avatar_appearance,
                object_media: patch.object_media,
                ..FacetUpdates::default()
            },
        };
        (patch.parent, fields)
    }

    pub(super) fn has_pose(&self) -> bool {
        self.pose.is_some_and(|pose| !pose.is_empty())
    }

    pub(super) fn orders_binding(&self) -> bool {
        self.has_pose()
            || !self.linear_motion.is_empty()
            || self.angular_velocity.is_some()
            || self.is_physical.is_some()
            || self.time_dilation.is_some()
            || self.scale.is_some()
            || self.attachment_point.is_present()
            || self.facets.orders_binding()
    }

    pub(super) fn has_facet_updates(&self) -> bool {
        !self.facets.is_empty()
    }

    pub(super) fn is_newer_than(&self, state: &ObjectState, fences: &Fences, facets: &FacetSet, kind: Kind, origin: Origin) -> bool {
        state.binding.address.region != origin.source.region
            || origin.stamp > state.binding.fence()
            || (self.has_pose() && origin.newer(fences.pose))
            || (!self.linear_motion.is_empty() && origin.newer(fences.linear_motion))
            || (self.angular_velocity.is_some() && origin.newer(fences.angular_velocity))
            || (self.is_physical.is_some() && origin.newer(fences.is_physical))
            || (self.time_dilation.is_some() && origin.newer(fences.time_dilation))
            || (self.scale.is_some() && origin.newer(fences.scale))
            || (self.attachment_point.is_present() && origin.newer(fences.attachment_point))
            || self.facets.has_newer(facets, kind, origin)
            || origin.newer(fences.parent)
    }

    pub(super) fn next_appearance(&self, facets: &FacetSet, origin: Origin) -> Option<Appearance> {
        self.facets.next_appearance(facets, origin)
    }

    pub(super) fn next_attachment_point(
        &self,
        current: Option<AttachmentPoint>,
        fences: &Fences,
        origin: Origin,
    ) -> Option<AttachmentPoint> {
        if self.attachment_point.is_present() && origin.newer(fences.attachment_point) {
            apply_field_update(current, self.attachment_point)
        } else {
            current
        }
    }

    pub(super) fn validate(&mut self, current_rotation: Option<Quat>) -> Rejections {
        let mut rejections = Rejections::new();
        if let Some(mut pose) = self.pose.filter(|pose| !pose.is_empty()) {
            let incoming_rotation = pose.rotation;
            let rotation = pose.rotation.map(normalized_rotation).transpose();
            match validate_vec3(pose.position, "position").err().or(rotation.err()) {
                Some(reason) => {
                    rejections.reject(FieldGroup::Pose, reason);
                    self.pose = None;
                }
                None => {
                    pose.rotation = rotation.expect("rotation was validated");
                    if let (Some(current), Some(incoming)) = (current_rotation, incoming_rotation)
                        && (incoming == current || is_exact_negation(current, incoming))
                    {
                        pose.rotation = Some(current);
                    }
                    self.pose = Some(pose);
                }
            }
        }
        let motion = self.linear_motion;
        let (velocity_limit, acceleration_limit) = match self.motion_update_kind {
            MotionUpdateKind::Full => (FULL_MOTION_COMPONENT_LIMIT, FULL_MOTION_COMPONENT_LIMIT),
            MotionUpdateKind::Terse => (TERSE_VELOCITY_COMPONENT_LIMIT, TERSE_ACCELERATION_COMPONENT_LIMIT),
        };
        if let Err(reason) = validate_motion_vec3(motion.velocity, "velocity", velocity_limit).and(validate_motion_vec3(
            motion.acceleration,
            "acceleration",
            acceleration_limit,
        )) {
            rejections.reject(FieldGroup::LinearMotion, reason);
            self.linear_motion = LinearMotion::default();
        }
        if let Err(reason) = validate_motion_vec3(self.angular_velocity, "angular_velocity", ANGULAR_VELOCITY_COMPONENT_LIMIT) {
            rejections.reject(FieldGroup::AngularVelocity, reason);
            self.angular_velocity = None;
        }
        if self.time_dilation.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
            rejections.reject(FieldGroup::TimeDilation, InvalidValue { field: "time_dilation" });
            self.time_dilation = None;
        }
        if let Err(reason) = validate_vec3(self.scale, "scale") {
            rejections.reject(FieldGroup::Scale, reason);
            self.scale = None;
        }
        if self.facets.object_media.as_ref().is_some_and(|update| update.validate().is_err()) {
            rejections.reject(FieldGroup::ObjectMedia, InvalidValue { field: "object_media" });
            self.facets.object_media = None;
        }
        rejections
    }

    pub(super) fn reject_unsupported(&mut self, kind: Kind, appearance: Option<Appearance>, rejections: &mut Rejections) {
        self.facets.reject_unsupported(kind, appearance, rejections);
    }

    pub(super) fn reject_parent(&mut self, cycle: bool, untrusted: bool, rejections: &mut Rejections) {
        if cycle {
            rejections.reject(FieldGroup::Parent, ParentCycle);
            if self.has_pose() {
                rejections.reject(FieldGroup::Pose, ParentDependsOnRejectedParent);
            }
            self.pose = None;
        } else if untrusted {
            rejections.reject(FieldGroup::Parent, UntrustedSource);
        }
    }

    pub(super) fn reject_untrusted(&mut self, fences: &Fences, facets: &FacetSet, rejections: &mut Rejections) {
        let tracked = |fence: Option<FieldStamp>| fence.is_some_and(|fence| fence.authority == Tracked);
        let groups = [
            (FieldGroup::Pose, self.has_pose(), tracked(fences.pose)),
            (FieldGroup::LinearMotion, !self.linear_motion.is_empty(), tracked(fences.linear_motion)),
            (FieldGroup::AngularVelocity, self.angular_velocity.is_some(), tracked(fences.angular_velocity)),
            (FieldGroup::Physical, self.is_physical.is_some(), tracked(fences.is_physical)),
            (FieldGroup::TimeDilation, self.time_dilation.is_some(), tracked(fences.time_dilation)),
            (FieldGroup::Scale, self.scale.is_some(), tracked(fences.scale)),
            (FieldGroup::AttachmentPoint, self.attachment_point.is_present(), tracked(fences.attachment_point)),
        ];
        for (group, present, protected) in groups {
            if present && protected {
                rejections.reject(group, UntrustedSource);
            }
        }
        self.facets.reject_untrusted(facets, UpdateAuthority::Untracked, rejections);
    }
}

#[derive(Clone, Copy)]
pub(super) struct Frame {
    changed: bool,
    generation: u64,
    fence: Option<FieldStamp>,
}

#[derive(Clone, Copy)]
pub(super) struct Settlement {
    pub(super) frame: Frame,
    pub(super) pose_accepted: bool,
    pub(super) detach_snapshot: Option<WorldPose>,
    pub(super) parent_applied: bool,
    pub(super) parent_cycle: bool,
    pub(super) advance_binding: bool,
}

pub(super) struct Draft {
    pub(super) state: ObjectState,
    pub(super) ordering: ObjectOrdering,
    base: FacetSet,
    facets: FacetSet,
    facet_changes: bool,
}

impl Draft {
    pub(super) fn new(state: ObjectState, ordering: ObjectOrdering, base: FacetSet) -> Self {
        Self { state, ordering, facets: base.clone(), base, facet_changes: false }
    }

    pub(super) fn reset_region_relative(&mut self) {
        self.facets.reset_region_relative();
        self.facet_changes = true;
    }

    pub(super) fn plan_frame(&self, changed: bool, origin: Origin) -> Result<Frame, ApplyError> {
        let generation = if changed {
            self.state.frame_generation.checked_add(1).ok_or(ApplyError::FrameGenerationExhausted)?
        } else {
            self.state.frame_generation
        };
        Ok(Frame { changed, generation, fence: if changed { Some(origin.field()) } else { self.ordering.fences.frame } })
    }

    pub(super) fn accept_pose(&self, fields: &Fields, origin: Origin, frame: Frame, rejections: &mut Rejections) -> bool {
        let candidate = fields.has_pose() && origin.newer(self.ordering.fences.pose);
        let current_frame = field_at_or_after(origin.field(), frame.fence);
        if candidate && !current_frame {
            rejections.reject(FieldGroup::Pose, StaleFrame);
        }
        candidate && current_frame
    }

    pub(super) fn pose_changes(&self, pose: Option<Pose>) -> bool {
        let Some(pose) = pose else { return false };
        let transform = self.state.transform;
        self.state.provisional_pose.is_some()
            || (pose.position.is_some() && self.state.position_frame_generation != Some(self.state.frame_generation))
            || (pose.rotation.is_some() && self.state.rotation_frame_generation != Some(self.state.frame_generation))
            || pose.position.is_some_and(|position| transform.position() != Some(position))
            || pose.rotation.is_some_and(|rotation| {
                transform.rotation().is_none_or(|current| current != rotation && !is_exact_negation(current, rotation))
            })
    }

    pub(super) fn update(
        &mut self,
        fields: Fields,
        kind: Kind,
        origin: Origin,
        settlement: Settlement,
        media_bytes: usize,
        rejections: &mut Rejections,
    ) {
        let Settlement { frame, pose_accepted, detach_snapshot, parent_applied, parent_cycle, advance_binding } = settlement;
        self.facet_changes |= fields.has_facet_updates();
        if frame.changed {
            self.state.frame_generation = frame.generation;
            self.state.provisional_pose = None;
            self.ordering.fences.frame = frame.fence;
        }
        let pose = fields.pose;
        self.apply(fields, kind, origin, pose_accepted, media_bytes, rejections);
        if pose_accepted {
            if let Some(pose) = pose {
                if pose.position.is_some() {
                    self.state.position_frame_generation = Some(self.state.frame_generation);
                }
                if pose.rotation.is_some() {
                    self.state.rotation_frame_generation = Some(self.state.frame_generation);
                }
            }
            self.state.provisional_pose = None;
        } else if let Some(snapshot) = detach_snapshot {
            self.state.provisional_pose = Some(snapshot);
        }
        if parent_applied {
            self.ordering.fences.parent = Some(origin.field());
            self.ordering.parent_rejection_barrier = None;
        } else if parent_cycle && origin.source.authority() == Tracked {
            self.ordering.parent_rejection_barrier = Some(ParentRejectionBarrier { region: origin.source.region, stamp: origin.stamp });
        }
        if advance_binding {
            self.state.binding.advance_fence(origin.source, origin.stamp);
        }
    }

    fn apply(&mut self, fields: Fields, kind: Kind, origin: Origin, pose_accepted: bool, media_bytes: usize, rejections: &mut Rejections) {
        let field = origin.field();
        let mut motion_observed = false;
        if let Some(pose) = fields.pose.filter(|_| pose_accepted) {
            if let Some(position) = pose.position {
                self.state.transform.position = Some(position);
            }
            if let Some(rotation) = pose.rotation
                && !self.state.transform.rotation.is_some_and(|current| is_exact_negation(current, rotation))
            {
                self.state.transform.rotation = Some(rotation);
            }
            self.ordering.fences.pose = Some(field);
            motion_observed = true;
        }
        if !fields.linear_motion.is_empty() && origin.newer(self.ordering.fences.linear_motion) {
            self.state.linear_motion.merge(fields.linear_motion);
            self.ordering.fences.linear_motion = Some(field);
            motion_observed = true;
        }
        if let Some(value) = fields.angular_velocity
            && origin.newer(self.ordering.fences.angular_velocity)
        {
            self.state.angular_velocity = Some(value);
            self.ordering.fences.angular_velocity = Some(field);
            motion_observed = true;
        }
        if let Some(value) = fields.is_physical
            && origin.newer(self.ordering.fences.is_physical)
        {
            self.state.is_physical = value;
            self.ordering.fences.is_physical = Some(field);
            motion_observed = true;
        }
        if let Some(value) = fields.time_dilation
            && origin.newer(self.ordering.fences.time_dilation)
        {
            self.state.time_dilation = value;
            self.ordering.fences.time_dilation = Some(field);
            motion_observed = true;
        }
        if let Some(value) = fields.scale
            && origin.newer(self.ordering.fences.scale)
        {
            self.state.transform.scale = Some(value);
            self.ordering.fences.scale = Some(field);
        }
        if fields.attachment_point.is_present() && origin.newer(self.ordering.fences.attachment_point) {
            self.state.attachment_point = apply_field_update(self.state.attachment_point, fields.attachment_point);
            self.ordering.fences.attachment_point = Some(field);
        }
        if motion_observed {
            self.state.motion_source = Some(origin.source);
        }
        if self.facet_changes {
            fields.facets.apply(&mut self.facets, origin, kind, media_bytes, rejections);
        }
    }

    pub(super) fn change_classes(&self, before: &ObjectState, after_parent: Parent) -> WorldChangeClasses {
        let after = &self.state;
        let mut classes = WorldChangeClasses::default();
        if before.transform.position() != after.transform.position()
            || before.transform.rotation() != after.transform.rotation()
            || before.frame_generation != after.frame_generation
            || before.position_frame_generation != after.position_frame_generation
            || before.rotation_frame_generation != after.rotation_frame_generation
            || before.provisional_pose != after.provisional_pose
            || before.parent != after_parent
        {
            classes |= WorldChangeClasses::POSE;
        }
        if before.linear_motion != after.linear_motion
            || before.angular_velocity != after.angular_velocity
            || before.is_physical != after.is_physical
            || before.time_dilation != after.time_dilation
            || before.motion_source != after.motion_source
        {
            classes |= WorldChangeClasses::MOTION;
        }
        if before.transform.scale() != after.transform.scale() {
            classes |= WorldChangeClasses::SCALE;
        }
        if before.parent != after_parent || before.attachment_point != after.attachment_point {
            classes |= WorldChangeClasses::RELATIONSHIP;
        }
        if self.facet_changes {
            classes |= self.facets.changed_classes(&self.base);
        }
        classes
    }

    pub(super) fn is_unchanged(&self, state: &ObjectState, ordering: &ObjectOrdering) -> bool {
        (!self.facet_changes || self.facets == self.base) && self.state == *state && self.ordering == *ordering
    }

    pub(super) fn media_bytes(&self) -> Option<(usize, usize)> {
        self.facet_changes.then(|| FacetChanges::new(self.base.clone(), self.facets.clone())).and_then(|changes| changes.media_bytes())
    }

    pub(super) fn into_parts(self) -> (ObjectState, Option<FacetChanges>, ObjectOrdering) {
        let changes = self.facet_changes.then(|| FacetChanges::new(self.base, self.facets));
        (self.state, changes, self.ordering)
    }
}

pub(super) fn validate_uuid(id: Uuid, field: &'static str) -> Result<(), ApplyError> {
    if id.is_nil() { Err(ApplyError::InvalidValue { field }) } else { Ok(()) }
}

pub(super) fn validate_vec3(value: Option<Vec3>, field: &'static str) -> Result<(), FieldRejectionReason> {
    if value.is_some_and(|value| !value.is_finite()) { Err(InvalidValue { field }) } else { Ok(()) }
}

fn validate_motion_vec3(value: Option<Vec3>, field: &'static str, limit: f32) -> Result<(), FieldRejectionReason> {
    if value.is_some_and(|value| !value.is_finite() || value.abs().max_element() > limit) { Err(InvalidValue { field }) } else { Ok(()) }
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

fn field_at_or_after(stamp: FieldStamp, fence: Option<FieldStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.authority) {
        (Tracked, Untracked) => true,
        (Untracked, Tracked) => false,
        _ => stamp.stamp >= fence.stamp,
    })
}

fn is_exact_negation(left: Quat, right: Quat) -> bool {
    left.x == -right.x && left.y == -right.y && left.z == -right.z && left.w == -right.w
}

fn apply_field_update<T: Copy>(current: Option<T>, update: FieldUpdate<T>) -> Option<T> {
    match update {
        FieldUpdate::Keep => current,
        FieldUpdate::Set(value) => Some(value),
        FieldUpdate::Clear => None,
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
