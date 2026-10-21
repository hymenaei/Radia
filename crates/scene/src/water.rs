use math::{Vec2, Vec3};

use crate::{Face, Mesh, MeshError, Vertex};

pub(crate) fn mesh(height: f32) -> Result<Mesh, MeshError> {
    let vertices = [(0.0, 0.0), (256.0, 0.0), (0.0, 256.0), (256.0, 256.0)]
        .map(|(x, y)| Vertex { position: Vec3::new(x, y, height), normal: Vec3::Z, uv: Vec2::new(x, y) / 256.0 })
        .to_vec();
    Mesh::from_parts(vertices, vec![0, 1, 3, 0, 3, 2], vec![Face { slot: 0, indices: 0..6 }])
}
