use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    mem::size_of,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use assets::mesh::MeshError;
use assets::{
    MeshAsset,
    mesh::{LodGeometry, Mesh, MeshLod},
};

use crate::scene::budget::Lease;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Detail {
    Lowest,
    Low,
    Medium,
    High,
}

impl Detail {
    pub(super) const fn scale(self) -> f32 {
        match self {
            Self::Lowest => 1.0,
            Self::Low => 1.5,
            Self::Medium => 2.5,
            Self::High => 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshBuildError {
    UnsupportedShape,
    InvalidParameters,
    TooComplex,
    Cancelled,
    Degenerate,
}

impl From<MeshError> for MeshBuildError {
    fn from(error: MeshError) -> Self {
        match error {
            MeshError::InvalidParameters => Self::InvalidParameters,
            MeshError::TooComplex => Self::TooComplex,
            MeshError::Degenerate => Self::Degenerate,
        }
    }
}

impl Display for MeshBuildError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedShape => "unsupported primitive shape",
            Self::InvalidParameters => "mesh generation parameters are invalid",
            Self::TooComplex => "generated geometry exceeds limits",
            Self::Cancelled => "mesh generation was cancelled",
            Self::Degenerate => "generated mesh contains no usable triangles",
        })
    }
}

impl Error for MeshBuildError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u64);

#[derive(Clone)]
pub struct SharedMesh(Arc<Retained>);

impl SharedMesh {
    pub(in crate::scene) fn new(mesh: Mesh, lease: Lease) -> Self {
        Self::retain(MeshSource::Owned(mesh), lease)
    }

    pub(in crate::scene) fn from_asset(asset: MeshAsset, lease: Lease) -> Option<Self> {
        matches!(asset.geometry(), LodGeometry::Geometry(_)).then(|| Self::retain(MeshSource::Asset(asset), lease))
    }

    fn retain(source: MeshSource, lease: Lease) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("mesh IDs exhausted");
        Self(Arc::new(Retained { id: MeshId(id), source, _lease: lease }))
    }

    pub fn id(&self) -> MeshId {
        self.0.id
    }

    pub(in crate::scene) fn downgrade(&self) -> WeakMesh {
        WeakMesh(Arc::downgrade(&self.0))
    }

    pub fn mesh(&self) -> &Mesh {
        match &self.0.source {
            MeshSource::Owned(mesh) => mesh,
            MeshSource::Asset(asset) => match asset.geometry() {
                LodGeometry::Geometry(mesh) => mesh,
                LodGeometry::Empty { .. } => unreachable!(),
            },
        }
    }
}

pub(in crate::scene) struct WeakMesh(Weak<Retained>);

impl WeakMesh {
    pub(in crate::scene) fn upgrade(&self) -> Option<SharedMesh> {
        self.0.upgrade().map(SharedMesh)
    }

    pub(in crate::scene) fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }
}

enum MeshSource {
    Owned(Mesh),
    Asset(MeshAsset),
}

struct Retained {
    id: MeshId,
    source: MeshSource,
    _lease: Lease,
}

pub(super) fn retained_bytes(mesh: &Mesh) -> usize {
    mesh.owned_bytes() + size_of::<Retained>() - size_of::<Mesh>() + 32
}

pub(in crate::scene) fn asset_mesh_bytes() -> usize {
    size_of::<Retained>() + 32
}

#[derive(Clone)]
pub(in crate::scene) struct MeshBinding {
    pub asset: utils::AssetId,
    pub lod: MeshLod,
    pub mesh: SharedMesh,
}
