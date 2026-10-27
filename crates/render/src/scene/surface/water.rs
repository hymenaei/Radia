use assets::mesh::{Face, Mesh, Vertex};

use crate::scene::MeshBuildError;
use math::{Vec2, Vec3};

pub(super) fn mesh(height: f32) -> Result<Mesh, MeshBuildError> {
    let vertices = [(0.0, 0.0), (256.0, 0.0), (0.0, 256.0), (256.0, 256.0)]
        .map(|(x, y)| Vertex { position: Vec3::new(x, y, height), normal: Vec3::Z, uv: Vec2::new(x, y) / 256.0, weights: [0.0; 4] })
        .to_vec();
    Mesh::from_parts(vertices, vec![0, 1, 3, 0, 3, 2], vec![Face { slot: 0, indices: 0..6 }]).map_err(MeshBuildError::from)
}
