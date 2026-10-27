mod asset;

pub use asset::{
    AssetError, MAX_MESH_ASSET_BYTES, MAX_MESH_BLOCK_BYTES, MAX_MESH_HEADER_BYTES, MESH_HEADER_BYTES, MeshHeader, MeshLod, MeshLodRequest,
    decode_header, decode_lod, decode_lod_cancellable, header_needs_more,
};

use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    mem::size_of,
    ops::Range,
};

use math::{Bounds, Vec2, Vec3};

pub const MAX_MESH_VERTICES: usize = 32_768;
pub const MAX_MESH_INDICES: usize = 196_608;
pub const MAX_MESH_FACES: usize = MAX_MESH_INDICES / 3;

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

#[derive(Debug)]
pub struct Mesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    faces: Vec<Face>,
    bounds: Bounds,
}

#[derive(Debug)]
pub enum LodGeometry {
    Geometry(Mesh),
    Empty { face_count: u8 },
}

impl Mesh {
    pub fn from_parts(vertices: Vec<Vertex>, indices: Vec<u32>, faces: Vec<Face>) -> Result<Self, MeshError> {
        if vertices.len() > MAX_MESH_VERTICES || indices.len() > MAX_MESH_INDICES || faces.len() > MAX_MESH_FACES {
            return Err(MeshError::TooComplex);
        }
        if indices.is_empty() {
            return Err(MeshError::Degenerate);
        }
        if !indices.len().is_multiple_of(3) || indices.iter().any(|&index| index as usize >= vertices.len()) {
            return Err(MeshError::InvalidParameters);
        }
        let index_count = indices.len() as u32;
        let mut covered_indices = 0;
        for face in &faces {
            if face.indices.start != covered_indices
                || face.indices.start > face.indices.end
                || face.indices.end > index_count
                || !face.indices.start.is_multiple_of(3)
                || !face.indices.end.is_multiple_of(3)
            {
                return Err(MeshError::InvalidParameters);
            }
            covered_indices = face.indices.end;
        }
        if covered_indices != index_count {
            return Err(MeshError::InvalidParameters);
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vertex in &vertices {
            if !vertex.position.is_finite() || !vertex.normal.is_finite() || !vertex.uv.is_finite() {
                return Err(MeshError::InvalidParameters);
            }
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshError {
    InvalidParameters,
    TooComplex,
    Degenerate,
}

impl Display for MeshError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidParameters => "mesh data is invalid",
            Self::TooComplex => "mesh exceeds geometry limits",
            Self::Degenerate => "mesh contains no usable triangles",
        })
    }
}

impl Error for MeshError {}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    fn vertex() -> Vertex {
        Vertex { position: Vec3::ZERO, normal: Vec3::Z, uv: Vec2::ZERO }
    }

    #[test]
    fn from_parts_rejects_invalid_indices() {
        let vertices = vec![vertex(); 3];
        let cases = [
            ("vertex index out of range", vec![0, 1, 3], vec![Face { slot: 0, indices: 0..3 }]),
            ("reversed face range", vec![0, 1, 2], vec![Face { slot: 0, indices: Range { start: 3, end: 0 } }]),
            ("face past index buffer", vec![0, 1, 2], vec![Face { slot: 0, indices: 0..4 }]),
            ("face leaves indices unused", vec![0, 1, 2], vec![Face { slot: 0, indices: 1..3 }]),
            ("too few indices", vec![0, 1], vec![]),
        ];
        for (case, indices, faces) in cases {
            assert_matches!(Mesh::from_parts(vertices.clone(), indices, faces).err(), Some(MeshError::InvalidParameters), "{case}");
        }
    }

    #[test]
    fn from_parts_requires_faces_to_cover_the_index_buffer() {
        let vertices = vec![vertex(); 6];
        let indices = vec![0, 1, 2, 3, 4, 5];

        for (case, faces) in [
            ("no faces", vec![]),
            ("gap", vec![Face { slot: 0, indices: 0..3 }, Face { slot: 1, indices: 4..6 }]),
            ("overlap", vec![Face { slot: 0, indices: 0..6 }, Face { slot: 1, indices: 3..6 }]),
        ] {
            assert_matches!(Mesh::from_parts(vertices.clone(), indices.clone(), faces).err(), Some(MeshError::InvalidParameters), "{case}");
        }
        let mesh = Mesh::from_parts(
            vertices.clone(),
            indices.clone(),
            vec![Face { slot: 0, indices: 0..0 }, Face { slot: 1, indices: 0..3 }, Face { slot: 2, indices: 3..6 }],
        )
        .unwrap();
        assert_eq!(mesh.faces().len(), 3, "an empty face may precede complete coverage");
        let mesh = Mesh::from_parts(vertices, indices, vec![Face { slot: 0, indices: 0..3 }, Face { slot: 1, indices: 3..6 }]).unwrap();
        assert_eq!(mesh.faces().len(), 2);
    }

    #[test]
    fn from_parts_rejects_nonfinite_vertices() {
        for (case, invalid) in [
            ("nan position", Vertex { position: Vec3::NAN, ..vertex() }),
            ("infinite normal", Vertex { normal: Vec3::splat(f32::INFINITY), ..vertex() }),
            ("infinite uv", Vertex { uv: Vec2::splat(f32::NEG_INFINITY), ..vertex() }),
        ] {
            assert_matches!(
                Mesh::from_parts(vec![vertex(), invalid, vertex()], vec![0, 1, 2], vec![]).err(),
                Some(MeshError::InvalidParameters),
                "{case}"
            );
        }
    }

    #[test]
    fn from_parts_enforces_mesh_limits() {
        assert_matches!(Mesh::from_parts(vec![vertex(); MAX_MESH_VERTICES + 1], vec![0, 1, 2], vec![]).err(), Some(MeshError::TooComplex));
        assert_matches!(Mesh::from_parts(vec![vertex(); 3], vec![0; MAX_MESH_INDICES + 1], vec![]).err(), Some(MeshError::TooComplex));
        assert_matches!(
            Mesh::from_parts(vec![vertex(); 3], vec![0, 1, 2], vec![Face { slot: 0, indices: 0..0 }; MAX_MESH_FACES + 1]).err(),
            Some(MeshError::TooComplex)
        );
        let mesh = Mesh::from_parts(
            vec![vertex(); MAX_MESH_VERTICES],
            vec![0; MAX_MESH_INDICES],
            vec![Face { slot: 0, indices: 0..MAX_MESH_INDICES as u32 }],
        )
        .unwrap();
        assert_eq!(mesh.vertices().len(), MAX_MESH_VERTICES);
        assert_eq!(mesh.indices().len(), MAX_MESH_INDICES);
    }

    #[test]
    fn from_parts_accepts_many_faces_with_reused_slots() {
        let face_count = 300;
        let indices = (0..face_count * 3).map(|index| u32::try_from(index).expect("test index fits u32")).collect();
        let faces = (0..face_count)
            .map(|index| {
                let start = u32::try_from(index * 3).expect("test face start fits u32");
                let end = u32::try_from((index + 1) * 3).expect("test face end fits u32");
                Face { slot: u8::try_from(index % (usize::from(u8::MAX) + 1)).expect("test face slot fits u8"), indices: start..end }
            })
            .collect();
        let mesh = Mesh::from_parts(vec![vertex(); face_count * 3], indices, faces).unwrap();

        assert_eq!(mesh.faces().len(), face_count);
    }
}
