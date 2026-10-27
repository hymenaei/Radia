use std::sync::Arc;

use math::Vec3;
use world::{Kind, ObjectDetail, PrimShape, Shape, WorldTransform, WorldTransformState};

use crate::material::FaceMaterial;
use crate::scene::{MeshKey, PreparedMesh, SharedMesh, asset_demand::Materials, budget::Lease};

#[derive(Clone)]
pub struct Object {
    data: Arc<ObjectData>,
    pub(super) preparation: Option<(MeshKey, PreparedMesh)>,
    pub(super) mesh: Option<(MeshKey, SharedMesh)>,
    pub(super) materials: Option<Materials>,
}

impl Object {
    pub(in crate::scene) fn new(source: ObjectDetail, lease: Lease) -> Self {
        Self { data: Arc::new(ObjectData { source, _lease: lease }), preparation: None, mesh: None, materials: None }
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
