use std::{sync::Arc, time::Instant};

use math::Vec3;
use world::{Kind, MotionUpdateKind, ObjectDetail, PrimShape, Shape, WorldTransform, WorldTransformState};

use crate::material::FaceMaterial;
use crate::scene::{
    MeshKey, PreparedMesh, SharedMesh,
    asset_demand::Materials,
    budget::Lease,
    motion::{MotionSample, MotionState},
    texture_animation,
};

#[derive(Clone)]
pub struct Object {
    data: Arc<ObjectData>,
    pub(super) preparation: Option<(MeshKey, PreparedMesh)>,
    pub(super) mesh: Option<(MeshKey, SharedMesh)>,
    pub(super) materials: Option<Materials>,
    animation_clock: Option<AnimationClock>,
    pub(super) motion: MotionState,
}

impl Object {
    pub(in crate::scene) fn new(source: ObjectDetail, lease: Lease) -> Self {
        let animation_clock = source.texture_animation.map(|_| AnimationClock { started_at: Instant::now(), frame_offset: 0.0 });
        let motion = MotionState::new(source.motion_source.and_then(world::UpdateSource::motion_received_at).unwrap_or_else(Instant::now));
        Self {
            data: Arc::new(ObjectData { source, _lease: lease }),
            preparation: None,
            mesh: None,
            materials: None,
            animation_clock,
            motion,
        }
    }

    pub fn detail(&self) -> &ObjectDetail {
        &self.data.source
    }

    pub fn mesh(&self) -> Option<(MeshKey, &SharedMesh)> {
        self.mesh.as_ref().map(|(key, mesh)| (*key, mesh))
    }

    pub fn materials(&self) -> Option<&[FaceMaterial]> {
        self.materials.as_ref().map(Materials::faces)
    }

    pub(crate) fn animation_timing(&self, now: Instant) -> Option<(f64, f64)> {
        self.animation_clock.map(|clock| (now.saturating_duration_since(clock.started_at).as_secs_f64(), clock.frame_offset))
    }

    pub(super) fn inherit_animation_timing(&mut self, old: &Self, now: Instant) {
        let Some(animation) = self.detail().texture_animation else {
            self.animation_clock = None;
            return;
        };
        let Some(previous) = old.detail().texture_animation else {
            return;
        };
        if animation == previous {
            self.animation_clock = old.animation_clock;
            return;
        }
        let frame_offset = if texture_animation::is_smooth(animation) && texture_animation::is_smooth(previous) {
            old.animation_timing(now)
                .and_then(|(elapsed, offset)| texture_animation::smooth_frame_counter(previous, elapsed, offset))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        self.animation_clock = Some(AnimationClock { started_at: now, frame_offset });
    }

    pub(super) fn same_appearance(&self, other: &Self) -> bool {
        let a = self.detail();
        let b = other.detail();
        a.summary.address.region() == b.summary.address.region()
            && self.geometry() == other.geometry()
            && a.surface_appearance == b.surface_appearance
            && a.pbr_materials == b.pbr_materials
            && a.pbr_material_overrides == b.pbr_material_overrides
    }

    pub(crate) fn same_render_appearance(&self, other: &Self) -> bool {
        self.same_appearance(other) && self.materials() == other.materials()
    }

    pub fn mesh_state(&self) -> Option<(MeshKey, &PreparedMesh)> {
        self.preparation.as_ref().map(|(key, state)| (*key, state))
    }

    pub fn placement(&self) -> Placement {
        let detail = self.detail();
        if detail.summary.kind == Kind::Avatar {
            return detail
                .world_transform
                .ok()
                .and_then(placeholder_avatar_transform)
                .map_or(Placement::Hidden(HiddenReason::UnresolvedPose), Placement::Ready);
        }
        let Ok(WorldTransformState::Resolved(transform)) = detail.world_transform else {
            return Placement::Hidden(HiddenReason::UnresolvedPose);
        };
        if !transform.scale.is_finite() || transform.scale.abs().min_element() == 0.0 {
            return Placement::Hidden(HiddenReason::InvalidDimensions);
        }
        Placement::Ready(transform)
    }

    pub(in crate::scene) fn motion_sample(&self) -> Option<MotionSample> {
        let detail = self.detail();
        let transform = match detail.world_transform {
            Ok(WorldTransformState::Resolved(transform)) => transform,
            Ok(WorldTransformState::MissingScale(pose)) => WorldTransform { pose, scale: Vec3::ONE },
            Ok(WorldTransformState::Provisional { .. } | WorldTransformState::Unresolved(_)) | Err(_) => return None,
        };
        let is_attachment = detail.attachment_point.is_some();
        Some(MotionSample {
            transform,
            kind: detail.summary.kind,
            region: detail.summary.address.region(),
            local_position: if is_attachment { Vec3::ZERO } else { detail.summary.local_transform.position().unwrap_or_default() },
            local_rotation: if is_attachment {
                transform.pose.rotation
            } else {
                detail.summary.local_transform.rotation().unwrap_or(transform.pose.rotation)
            },
            velocity: detail.linear_motion.velocity.unwrap_or_default(),
            acceleration: detail.linear_motion.acceleration.unwrap_or_default(),
            angular_velocity: detail.angular_velocity.unwrap_or_default(),
            is_physical: detail.is_physical,
            time_dilation: detail.time_dilation,
            ping_delay: detail.motion_source.and_then(world::UpdateSource::motion_ping_delay),
            motion_update_kind: detail.motion_source.map_or(MotionUpdateKind::Full, world::UpdateSource::motion_update_kind),
            is_attachment,
            source: detail.motion_source,
        })
    }

    pub(in crate::scene) fn motion_state(&self) -> MotionState {
        self.motion
    }

    pub(crate) fn motion_changed_from(&self, previous: &Self) -> bool {
        match (previous.motion_sample(), self.motion_sample()) {
            (Some(previous), Some(next)) => crate::scene::motion::motion_changed(previous, next),
            (None, None) => false,
            _ => true,
        }
    }

    pub(in crate::scene) fn start_motion_phase_out(&mut self, at: Option<Instant>) {
        if let Some(sample) = self.motion_sample() {
            self.motion.start_linear_phase_out(sample, at);
        }
    }

    pub(in crate::scene) fn resume_motion_at(&mut self, now: Instant, regions: &[math::motion::RegionBounds]) {
        if let Some(sample) = self.motion_sample() {
            self.motion.resume_linear_at(sample, now, regions);
        }
    }

    pub(super) fn update_motion_from(&mut self, previous: &Self, at: Instant) {
        let (Some(previous_sample), Some(next_sample)) = (previous.motion_sample(), self.motion_sample()) else {
            self.motion = MotionState::new(at);
            return;
        };
        let sampled_at = next_sample.source.and_then(world::UpdateSource::motion_received_at).unwrap_or(at);
        self.motion = MotionState::updated_at(previous.motion, previous_sample, next_sample, at, sampled_at);
    }

    pub fn geometry(&self) -> Geometry {
        if self.detail().summary.kind == Kind::Avatar {
            return Geometry::Prim(PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() });
        }
        match self.detail().shape {
            Some(Shape::Prim(shape)) => Geometry::Prim(shape),
            Some(shape) => Geometry::Unsupported(shape),
            None => Geometry::Missing,
        }
    }
}

fn placeholder_avatar_transform(transform: WorldTransformState) -> Option<WorldTransform> {
    let mut transform = match transform {
        WorldTransformState::Resolved(transform) => transform,
        WorldTransformState::MissingScale(pose) => WorldTransform { pose, scale: Vec3::new(0.6, 0.45, 1.9) },
        WorldTransformState::Provisional { .. } | WorldTransformState::Unresolved(_) => return None,
    };
    transform.scale = Vec3::new(0.6, 0.45, 1.9);
    Some(transform)
}

#[derive(Clone, Copy)]
struct AnimationClock {
    started_at: Instant,
    frame_offset: f64,
}

struct ObjectData {
    source: ObjectDetail,
    _lease: Lease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenReason {
    Avatar,
    UnresolvedPose,
    InvalidDimensions,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    Ready(WorldTransform),
    Hidden(HiddenReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    Prim(PrimShape),
    Unsupported(Shape),
    Missing,
}

#[cfg(test)]
mod tests {
    use math::{DVec3, Quat};
    use world::{WorldPose, WorldTransform};

    use super::placeholder_avatar_transform;
    use world::WorldTransformState;

    #[test]
    fn placeholder_avatar_uses_world_pose_and_default_agent_dimensions() {
        let pose = WorldPose { position: DVec3::new(10.0, 20.0, 30.0), rotation: Quat::from_rotation_z(0.5) };
        let transform = placeholder_avatar_transform(WorldTransformState::MissingScale(pose)).expect("placeholder transform");
        assert_eq!(transform, WorldTransform { pose, scale: math::Vec3::new(0.6, 0.45, 1.9) });
        assert!(
            placeholder_avatar_transform(WorldTransformState::Provisional { pose, reason: world::UnresolvedReason::PendingParent })
                .is_none()
        );
    }
}
