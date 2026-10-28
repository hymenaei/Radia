use std::{sync::Arc, time::Instant};

use math::Vec3;
use utils::AssetId;
use world::{Kind, ObjectDetail, PrimShape, SculptFlags, Shape, WorldTransform, WorldTransformState};

use crate::material::FaceMaterial;
use crate::scene::{MeshKey, PreparedMesh, SharedMesh, asset_demand::Materials, budget::Lease, mesh::MeshBinding, texture_animation};

#[derive(Clone)]
pub struct Object {
    data: Arc<ObjectData>,
    pub(super) preparation: Option<(MeshKey, PreparedMesh)>,
    pub(super) mesh: Option<(MeshKey, SharedMesh)>,
    pub(super) asset_mesh: Option<MeshBinding>,
    pub(super) materials: Option<Materials>,
    animation_clock: Option<AnimationClock>,
}

impl Object {
    pub(in crate::scene) fn new(source: ObjectDetail, lease: Lease) -> Self {
        let animation_clock = source.texture_animation.map(|_| AnimationClock { started_at: Instant::now(), frame_offset: 0.0 });
        Self {
            data: Arc::new(ObjectData { source, _lease: lease }),
            preparation: None,
            mesh: None,
            asset_mesh: None,
            materials: None,
            animation_clock,
        }
    }

    pub fn detail(&self) -> &ObjectDetail {
        &self.data.source
    }

    pub fn mesh(&self) -> Option<(MeshKey, &SharedMesh)> {
        self.mesh.as_ref().map(|(key, mesh)| (*key, mesh))
    }

    pub fn render_mesh(&self) -> Option<&SharedMesh> {
        match self.geometry() {
            Geometry::Prim(_) => self.mesh.as_ref().map(|(_, mesh)| mesh),
            Geometry::Mesh(asset) => self.asset_mesh.as_ref().filter(|binding| binding.asset == asset).map(|binding| &binding.mesh),
            Geometry::Unsupported(_) | Geometry::Missing => None,
        }
    }

    pub fn mesh_lod(&self) -> Option<(AssetId, assets::mesh::MeshLod)> {
        self.asset_mesh.as_ref().and_then(|binding| match self.geometry() {
            Geometry::Mesh(asset) if asset == binding.asset => Some((binding.asset, binding.lod)),
            _ => None,
        })
    }

    pub fn mesh_flags(&self) -> Option<SculptFlags> {
        match self.detail().shape {
            Some(Shape::Mesh { flags, .. }) => Some(flags),
            _ => None,
        }
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

    pub fn geometry(&self) -> Geometry {
        if self.detail().summary.kind == Kind::Avatar {
            return Geometry::Prim(PrimShape { path_curve: 0x20, profile_curve: 5, ..PrimShape::default() });
        }
        match self.detail().shape {
            Some(Shape::Prim(shape)) => Geometry::Prim(shape),
            Some(Shape::Mesh { asset_id, .. }) => Geometry::Mesh(asset_id),
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
    Mesh(AssetId),
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
