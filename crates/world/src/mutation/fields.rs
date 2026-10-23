use std::sync::Arc;

use math::{Quat, Vec3};
use uuid::Uuid;

use crate::history::WorldChangeClasses;
use crate::update::{FieldStamp, Stamp};
use crate::{
    Appearance, ApplyError, AttachmentPoint, AvatarAppearanceUpdate, FieldGroup, FieldRejectionReason, FieldUpdate, Kind, LinearMotion,
    ObjectMedia, ObjectMediaUpdate, ObjectObservation, ObjectPatch, Parent, ParentUpdate, PbrMaterialAssignments, PbrMaterialOverrides,
    Pose, Rejections, Shape, SurfaceAppearance, UpdateAuthority, UpdateSource, WorldPose,
};
use crate::{Fences, ObjectContent, ObjectOrdering, ObjectState, ParentRejectionBarrier, RegionalStamp, newer_field, newer_region_field};
use FieldRejectionReason::{
    InvalidRotation, InvalidValue, ParentCycle, ParentDependsOnRejectedParent, ResourceLimit, StaleFrame, UntrustedSource,
};
use UpdateAuthority::{Tracked, Untracked};

#[derive(Clone, Copy)]
pub(super) struct Origin {
    pub(super) source: UpdateSource,
    pub(super) stamp: Stamp,
}

impl Origin {
    pub(super) fn field(self) -> FieldStamp {
        FieldStamp { authority: self.source.authority(), stamp: self.stamp }
    }

    fn regional(self) -> RegionalStamp {
        RegionalStamp { region: self.source.region, stamp: self.field() }
    }

    fn newer(self, fence: Option<FieldStamp>) -> bool {
        newer_field(self.field(), fence)
    }

    fn newer_regional(self, fence: Option<RegionalStamp>) -> bool {
        newer_region_field(self.source.region, self.field(), fence)
    }
}

pub(super) struct Fields {
    pub(super) pose: Option<Pose>,
    linear_motion: LinearMotion,
    angular_velocity: Option<Vec3>,
    scale: Option<Vec3>,
    pub(super) attachment_point: FieldUpdate<AttachmentPoint>,
    shape: FieldUpdate<Shape>,
    pub(super) appearance: FieldUpdate<Appearance>,
    surface_appearance: Option<SurfaceAppearance>,
    pbr_materials: Option<PbrMaterialAssignments>,
    pbr_material_overrides: Option<PbrMaterialOverrides>,
    pub(super) avatar_appearance: Option<AvatarAppearanceUpdate>,
    object_media: Option<ObjectMediaUpdate>,
}

impl Fields {
    pub(super) fn observed(observation: ObjectObservation) -> Self {
        Self {
            pose: observation.pose,
            linear_motion: observation.linear_motion.unwrap_or_default(),
            angular_velocity: observation.angular_velocity,
            scale: observation.scale,
            attachment_point: observation.attachment_point,
            shape: observation.shape,
            appearance: observation.appearance,
            surface_appearance: observation.surface_appearance,
            pbr_materials: observation.pbr_materials,
            pbr_material_overrides: None,
            avatar_appearance: None,
            object_media: None,
        }
    }

    pub(super) fn patched(patch: ObjectPatch) -> (Option<ParentUpdate>, Self) {
        let fields = Self {
            pose: patch.pose,
            linear_motion: patch.linear_motion,
            angular_velocity: patch.angular_velocity,
            scale: patch.scale,
            attachment_point: patch.attachment_point,
            shape: patch.shape,
            appearance: patch.appearance,
            surface_appearance: patch.surface_appearance,
            pbr_materials: patch.pbr_materials,
            pbr_material_overrides: patch.pbr_material_overrides,
            avatar_appearance: patch.avatar_appearance,
            object_media: patch.object_media,
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
            || self.scale.is_some()
            || self.attachment_point.is_present()
            || self.shape.is_present()
            || self.appearance.is_present()
            || self.surface_appearance.is_some()
            || self.pbr_materials.is_some()
            || self.pbr_material_overrides.is_some()
            || self.avatar_appearance.is_some()
    }

    pub(super) fn is_newer_than(&self, state: &ObjectState, fences: &Fences, kind: Kind, origin: Origin) -> bool {
        state.binding.address.region != origin.source.region
            || origin.stamp > state.binding.fence()
            || (self.has_pose() && origin.newer(fences.pose))
            || (!self.linear_motion.is_empty() && origin.newer(fences.linear_motion))
            || (self.angular_velocity.is_some() && origin.newer(fences.angular_velocity))
            || (self.scale.is_some() && origin.newer(fences.scale))
            || (self.attachment_point.is_present() && origin.newer(fences.attachment_point))
            || (self.shape.is_present() && origin.newer_regional(fences.shape))
            || (self.appearance.is_present() && origin.newer_regional(fences.appearance))
            || (kind != Kind::Avatar && self.surface_appearance.is_some() && origin.newer_regional(fences.texture_entry))
            || (kind != Kind::Avatar && self.pbr_materials.is_some() && origin.newer_regional(fences.pbr_materials))
            || origin.newer(fences.parent)
    }

    pub(super) fn next_appearance(&self, current: Option<Appearance>, fences: &Fences, origin: Origin) -> Option<Appearance> {
        if self.appearance.is_present() && origin.newer_regional(fences.appearance) {
            apply_field_update(current, self.appearance)
        } else {
            current
        }
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
        if let Err(reason) = validate_vec3(motion.velocity, "velocity").and(validate_vec3(motion.acceleration, "acceleration")) {
            rejections.reject(FieldGroup::LinearMotion, reason);
            self.linear_motion = LinearMotion::default();
        }
        if let Err(reason) = validate_vec3(self.angular_velocity, "angular_velocity") {
            rejections.reject(FieldGroup::AngularVelocity, reason);
            self.angular_velocity = None;
        }
        if let Err(reason) = validate_vec3(self.scale, "scale") {
            rejections.reject(FieldGroup::Scale, reason);
            self.scale = None;
        }
        if self.object_media.as_ref().is_some_and(|update| update.validate().is_err()) {
            rejections.reject(FieldGroup::ObjectMedia, InvalidValue { field: "object_media" });
            self.object_media = None;
        }
        rejections
    }

    pub(super) fn reject_unsupported(&mut self, kind: Kind, appearance: Option<Appearance>, rejections: &mut Rejections) {
        let accepts_surface = kind == Kind::Object && appearance == Some(Appearance::Surface);
        if self.object_media.is_some() && !accepts_surface {
            rejections.reject(FieldGroup::ObjectMedia, InvalidValue { field: "object_media" });
            self.object_media = None;
        }
        if self.avatar_appearance.is_some() && kind != Kind::Avatar {
            rejections.reject(FieldGroup::AvatarAppearance, InvalidValue { field: "avatar_appearance" });
            self.avatar_appearance = None;
        }
        if self.pbr_material_overrides.as_ref().is_some_and(|overrides| !overrides.is_empty()) && !accepts_surface {
            rejections.reject(FieldGroup::PbrMaterialOverrides, InvalidValue { field: "pbr_material_overrides" });
            self.pbr_material_overrides = None;
        }
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

    pub(super) fn reject_untrusted(&self, fences: &Fences, rejections: &mut Rejections) {
        let tracked = |fence: Option<FieldStamp>| fence.is_some_and(|fence| fence.authority == Tracked);
        let regional = |fence: Option<RegionalStamp>| tracked(fence.map(|fence| fence.stamp));
        let groups = [
            (FieldGroup::Pose, self.has_pose(), tracked(fences.pose)),
            (FieldGroup::LinearMotion, !self.linear_motion.is_empty(), tracked(fences.linear_motion)),
            (FieldGroup::AngularVelocity, self.angular_velocity.is_some(), tracked(fences.angular_velocity)),
            (FieldGroup::Scale, self.scale.is_some(), tracked(fences.scale)),
            (FieldGroup::AttachmentPoint, self.attachment_point.is_present(), tracked(fences.attachment_point)),
            (FieldGroup::Shape, self.shape.is_present(), regional(fences.shape)),
            (FieldGroup::Appearance, self.appearance.is_present(), regional(fences.appearance)),
            (FieldGroup::TextureEntry, self.surface_appearance.is_some(), regional(fences.texture_entry)),
            (FieldGroup::PbrMaterials, self.pbr_materials.is_some(), regional(fences.pbr_materials)),
            (FieldGroup::PbrMaterialOverrides, self.pbr_material_overrides.is_some(), regional(fences.pbr_material_overrides)),
            (FieldGroup::AvatarAppearance, self.avatar_appearance.is_some(), regional(fences.avatar_appearance)),
            (FieldGroup::ObjectMedia, self.object_media.is_some(), true),
        ];
        for (group, present, protected) in groups {
            if present && protected {
                rejections.reject(group, UntrustedSource);
            }
        }
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

pub(super) struct Draft<'a> {
    pub(super) state: ObjectState,
    pub(super) ordering: ObjectOrdering,
    base: &'a ObjectContent,
    content: Option<ObjectContent>,
}

impl<'a> Draft<'a> {
    pub(super) const fn new(state: ObjectState, ordering: ObjectOrdering, base: &'a ObjectContent) -> Self {
        Self { state, ordering, base, content: None }
    }

    pub(super) fn content(&self) -> &ObjectContent {
        self.content.as_ref().unwrap_or(self.base)
    }

    pub(super) fn content_mut(&mut self) -> &mut ObjectContent {
        self.content.get_or_insert_with(|| self.base.clone())
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
        let regional = origin.regional();
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
        }
        if !fields.linear_motion.is_empty() && origin.newer(self.ordering.fences.linear_motion) {
            self.state.linear_motion.merge(fields.linear_motion);
            self.ordering.fences.linear_motion = Some(field);
        }
        if let Some(value) = fields.angular_velocity
            && origin.newer(self.ordering.fences.angular_velocity)
        {
            self.state.angular_velocity = Some(value);
            self.ordering.fences.angular_velocity = Some(field);
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
        if fields.shape.is_present() && origin.newer_regional(self.ordering.fences.shape) {
            let shape = apply_field_update(self.content().shape, fields.shape);
            self.content_mut().shape = shape;
            self.ordering.fences.shape = Some(regional);
        }
        if fields.appearance.is_present() && origin.newer_regional(self.ordering.fences.appearance) {
            let appearance = apply_field_update(self.content().appearance, fields.appearance);
            self.content_mut().appearance = appearance;
            self.ordering.fences.appearance = Some(regional);
        }
        if let Some(value) = fields.surface_appearance
            && kind != Kind::Avatar
            && origin.newer_regional(self.ordering.fences.texture_entry)
        {
            if self.content().surface_appearance.as_deref() != Some(&value) {
                self.content_mut().surface_appearance = Some(Arc::new(value));
            }
            self.ordering.fences.texture_entry = Some(regional);
        }
        if let Some(value) = fields.pbr_materials
            && kind != Kind::Avatar
            && origin.newer_regional(self.ordering.fences.pbr_materials)
        {
            let value = (!value.is_empty()).then_some(value);
            if self.content().pbr_materials.as_deref() != value.as_ref() {
                self.content_mut().pbr_materials = value.map(Arc::new);
            }
            self.ordering.fences.pbr_materials = Some(regional);
        }
        if let Some(value) = fields.pbr_material_overrides
            && origin.newer_regional(self.ordering.fences.pbr_material_overrides)
        {
            let value = (!value.is_empty()).then_some(value);
            if self.content().pbr_material_overrides.as_deref() != value.as_ref() {
                self.content_mut().pbr_material_overrides = value.map(Arc::new);
            }
            self.ordering.fences.pbr_material_overrides = Some(regional);
        }
        if let Some(update) = fields.avatar_appearance
            && origin.newer_regional(self.ordering.fences.avatar_appearance)
        {
            let mut appearance = self.content().avatar_appearance.as_deref().cloned().unwrap_or_default();
            appearance.apply(update);
            self.content_mut().avatar_appearance = Some(Arc::new(appearance));
            self.ordering.fences.avatar_appearance = Some(regional);
        }
        if let Some(update) = fields.object_media
            && origin.source.authority() == Tracked
            && self.content().object_media.as_deref().is_none_or(|current| update.version() > current.version())
        {
            let current = self.content().object_media.as_deref();
            let current_bytes = current.map_or(0, ObjectMedia::retained_bytes);
            let mut media = current.cloned().unwrap_or_default();
            media.apply(&update);
            if media_bytes.saturating_sub(current_bytes).saturating_add(media.retained_bytes()) > crate::MAX_OBJECT_MEDIA_BYTES {
                rejections.reject(FieldGroup::ObjectMedia, ResourceLimit { field: "object_media" });
            } else {
                self.content_mut().object_media = Some(Arc::new(media));
            }
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
        if before.linear_motion != after.linear_motion || before.angular_velocity != after.angular_velocity {
            classes |= WorldChangeClasses::MOTION;
        }
        if before.transform.scale() != after.transform.scale() {
            classes |= WorldChangeClasses::SCALE;
        }
        if before.parent != after_parent || before.attachment_point != after.attachment_point {
            classes |= WorldChangeClasses::RELATIONSHIP;
        }
        if let Some(content) = &self.content {
            classes |= content_change_classes(self.base, content);
        }
        classes
    }

    pub(super) fn is_unchanged(&self, state: &ObjectState, ordering: &ObjectOrdering) -> bool {
        self.content.is_none() && self.state == *state && self.ordering == *ordering
    }

    pub(super) fn media_bytes(&self) -> Option<(usize, usize)> {
        let content = self.content.as_ref()?;
        let bytes = |content: &ObjectContent| content.object_media.as_deref().map_or(0, ObjectMedia::retained_bytes);
        Some((bytes(self.base), bytes(content)))
    }

    pub(super) fn into_parts(self) -> (ObjectState, Option<ObjectContent>, ObjectOrdering) {
        (self.state, self.content, self.ordering)
    }
}

fn content_change_classes(before: &ObjectContent, after: &ObjectContent) -> WorldChangeClasses {
    let mut classes = WorldChangeClasses::default();
    if before.sit_target != after.sit_target {
        classes |= WorldChangeClasses::RELATIONSHIP;
    }
    if before.shape != after.shape {
        classes |= WorldChangeClasses::SHAPE;
    }
    if before.appearance != after.appearance
        || !same_arc(before.surface_appearance.as_ref(), after.surface_appearance.as_ref())
        || !same_arc(before.pbr_materials.as_ref(), after.pbr_materials.as_ref())
        || !same_arc(before.pbr_material_overrides.as_ref(), after.pbr_material_overrides.as_ref())
        || !same_arc(before.avatar_appearance.as_ref(), after.avatar_appearance.as_ref())
    {
        classes |= WorldChangeClasses::APPEARANCE;
    }
    if !same_arc(before.object_media.as_ref(), after.object_media.as_ref()) {
        classes |= WorldChangeClasses::MEDIA;
    }
    if before.properties != after.properties || before.properties_provenance != after.properties_provenance {
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

pub(super) fn validate_uuid(id: Uuid, field: &'static str) -> Result<(), ApplyError> {
    if id.is_nil() { Err(ApplyError::InvalidValue { field }) } else { Ok(()) }
}

pub(super) fn validate_vec3(value: Option<Vec3>, field: &'static str) -> Result<(), FieldRejectionReason> {
    if value.is_some_and(|value| !value.is_finite()) { Err(InvalidValue { field }) } else { Ok(()) }
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
