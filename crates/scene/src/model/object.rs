use std::sync::Arc;

use world::{Kind, ObjectDetail, PrimShape, Shape, WorldTransform, WorldTransformState};

use crate::{FaceMaterial, MeshKey, PreparedMesh, SharedMesh, asset_demand::Materials, budget::Lease};

#[derive(Clone)]
pub struct Object {
    data: Arc<ObjectData>,
    pub(super) preparation: Option<(MeshKey, PreparedMesh)>,
    pub(super) mesh: Option<(MeshKey, SharedMesh)>,
    pub(super) materials: Option<Materials>,
}

impl Object {
    pub(crate) fn new(source: ObjectDetail, lease: Lease) -> Self {
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
            && a.render_materials == b.render_materials
            && a.gltf_material_overrides == b.gltf_material_overrides
    }

    pub fn mesh_state(&self) -> Option<(MeshKey, &PreparedMesh)> {
        self.preparation.as_ref().map(|(key, state)| (*key, state))
    }

    pub fn placement(&self) -> Placement {
        let detail = self.detail();
        if detail.summary.kind == Kind::Avatar {
            return Placement::Hidden(HiddenReason::Avatar);
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
        match self.detail().shape {
            Some(Shape::Prim(shape)) => Geometry::Prim(shape),
            Some(shape) => Geometry::Unsupported(shape),
            None => Geometry::Missing,
        }
    }
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
