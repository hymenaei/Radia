use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use math::{Vec2, Vec3};

use crate::budget::Lease;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u64);

#[derive(Clone)]
pub struct SharedMesh(Arc<Retained>);

struct Retained {
    id: MeshId,
    mesh: Mesh,
    _lease: Lease,
}

impl SharedMesh {
    pub(crate) fn new(mesh: Mesh, lease: Lease) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("mesh IDs exhausted");
        Self(Arc::new(Retained { id: MeshId(id), mesh, _lease: lease }))
    }

    pub fn id(&self) -> MeshId {
        self.0.id
    }

    pub fn mesh(&self) -> &Mesh {
        &self.0.mesh
    }
}

pub const MAX_MESH_VERTICES: usize = 32_768;
pub const MAX_MESH_INDICES: usize = 196_608;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Face {
    pub slot: u8,
    pub indices: Range<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

pub struct Mesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    faces: Vec<Face>,
    bounds: Bounds,
}

impl Mesh {
    pub(crate) fn from_parts(vertices: Vec<Vertex>, indices: Vec<u32>, faces: Vec<Face>) -> Result<Self, MeshError> {
        if indices.is_empty() {
            return Err(MeshError::Degenerate);
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vertex in &vertices {
            min = min.min(vertex.position);
            max = max.max(vertex.position);
        }
        Ok(Self { vertices, indices, faces, bounds: Bounds { min, max } })
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
    pub fn faces(&self) -> &[Face] {
        &self.faces
    }
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }
    pub fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            + self.vertices.capacity() * size_of::<Vertex>()
            + self.indices.capacity() * size_of::<u32>()
            + self.faces.capacity() * size_of::<Face>()
    }

    pub(crate) fn shared_bytes(&self) -> usize {
        self.owned_bytes() + size_of::<Retained>() - size_of::<Self>() + 32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshError {
    UnsupportedCurve,
    InvalidParameters,
    TooComplex,
    Cancelled,
    Degenerate,
}

impl Display for MeshError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedCurve => "unsupported procedural profile or path",
            Self::InvalidParameters => "invalid procedural volume parameters",
            Self::TooComplex => "procedural volume exceeds geometry limits",
            Self::Cancelled => "procedural volume generation was cancelled",
            Self::Degenerate => "procedural volume contains no usable triangles",
        })
    }
}

impl Error for MeshError {}

#[derive(Default)]
pub(crate) struct Builder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    faces: Vec<Face>,
    parents: Vec<usize>,
}

impl Builder {
    pub(crate) fn vertex(&mut self, position: Vec3, uv: Vec2) -> Result<usize, MeshError> {
        if !position.is_finite() || !uv.is_finite() {
            return Err(MeshError::InvalidParameters);
        }
        if self.vertices.len() == MAX_MESH_VERTICES {
            return Err(MeshError::TooComplex);
        }
        let index = self.vertices.len();
        self.vertices.push(Vertex { position, normal: Vec3::ZERO, uv });
        self.parents.push(index);
        Ok(index)
    }

    pub(crate) fn triangle(&mut self, a: usize, b: usize, c: usize) -> Result<(), MeshError> {
        if self.indices.len() + 3 > MAX_MESH_INDICES {
            return Err(MeshError::TooComplex);
        }
        let cross = (self.vertices[b].position - self.vertices[a].position).cross(self.vertices[c].position - self.vertices[a].position);
        if cross.length_squared() > 1e-20 {
            self.indices.extend([a as u32, b as u32, c as u32]);
        }
        Ok(())
    }

    pub(crate) fn begin_face(&mut self) -> usize {
        self.indices.len()
    }

    pub(crate) fn end_face(&mut self, start: usize) -> Result<(), MeshError> {
        if self.faces.len() == 9 {
            return Err(MeshError::TooComplex);
        }
        self.faces.push(Face { slot: self.faces.len() as u8, indices: start as u32..self.indices.len() as u32 });
        Ok(())
    }

    pub(crate) fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub(crate) fn seam(&mut self, a: usize, b: usize) {
        if self.vertices[a].position.distance_squared(self.vertices[b].position) <= 1e-12 {
            let a = root(&mut self.parents, a);
            let b = root(&mut self.parents, b);
            self.parents[b] = a;
        }
    }

    pub(crate) fn finish(mut self, cancelled: &AtomicBool) -> Result<Mesh, MeshError> {
        if self.indices.is_empty() {
            return Err(MeshError::Degenerate);
        }
        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for (ordinal, triangle) in self.indices.as_chunks::<3>().0.iter().enumerate() {
            if ordinal % 256 == 0 && cancelled.load(Ordering::Acquire) {
                return Err(MeshError::Cancelled);
            }
            let [a, b, c] = [triangle[0] as usize, triangle[1] as usize, triangle[2] as usize];
            let normal =
                (self.vertices[b].position - self.vertices[a].position).cross(self.vertices[c].position - self.vertices[a].position);
            for index in [a, b, c] {
                normals[root(&mut self.parents, index)] += normal;
            }
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for (index, vertex) in self.vertices.iter_mut().enumerate() {
            vertex.normal = normals[root(&mut self.parents, index)].try_normalize().unwrap_or(Vec3::Z);
            min = min.min(vertex.position);
            max = max.max(vertex.position);
        }
        Ok(Mesh { vertices: self.vertices, indices: self.indices, faces: self.faces, bounds: Bounds { min, max } })
    }
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}
