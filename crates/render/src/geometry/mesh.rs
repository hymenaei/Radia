use math::Vec3;
use wgpu::Device;

use super::{GeometryRenderer, MAX_MESH_UPLOADS, MAX_MESHES, UPLOAD_BYTES};
use crate::geometry::storage::Allocation;
use crate::pipeline::VERTEX_BYTES;
use crate::scene::SharedMesh;

pub(super) struct GpuMesh {
    pub(super) allocation: Allocation,
    pub(super) centers: Vec<Vec3>,
    pub(super) used: u64,
}

impl GeometryRenderer {
    pub(super) fn upload_mesh(&mut self, device: &Device, shared: &SharedMesh, uploaded: &mut usize) -> bool {
        let key = shared.id();
        if self.meshes.contains_key(&key) {
            return true;
        }
        let mesh = shared.mesh();
        let Some(bytes) = mesh
            .vertices()
            .len()
            .checked_mul(VERTEX_BYTES)
            .and_then(|vertices| mesh.indices().len().checked_mul(4).and_then(|indices| vertices.checked_add(indices)))
        else {
            return false;
        };
        if *uploaded >= MAX_MESH_UPLOADS || bytes > UPLOAD_BYTES.saturating_sub(self.stats.uploaded_bytes) {
            return false;
        }
        if self.meshes.len() >= MAX_MESHES && !self.retire_mesh() {
            return false;
        }
        let Some(allocation) = self.storage.allocate(device, mesh.vertices().len(), mesh.indices().len(), &self.memory) else {
            self.retire_mesh();
            return false;
        };
        let mut packed = Vec::with_capacity(bytes);
        for vertex in mesh.vertices() {
            floats(&mut packed, vertex.position.to_array());
            floats(&mut packed, vertex.normal.to_array());
            floats(&mut packed, vertex.uv.to_array());
        }
        self.queue.write_buffer(&allocation.buffers().vertices, u64::from(allocation.vertices().start) * VERTEX_BYTES as u64, &packed);
        packed.clear();
        for index in mesh.indices() {
            packed.extend_from_slice(&index.to_le_bytes());
        }
        self.queue.write_buffer(&allocation.buffers().indices, u64::from(allocation.indices().start) * size_of::<u32>() as u64, &packed);
        let centers = mesh
            .faces()
            .iter()
            .map(|face| {
                if face.indices.is_empty() {
                    return Vec3::ZERO;
                }
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for index in &mesh.indices()[face.indices.start as usize..face.indices.end as usize] {
                    let p = mesh.vertices()[*index as usize].position;
                    min = min.min(p);
                    max = max.max(p);
                }
                min * 0.5 + max * 0.5
            })
            .collect();
        self.meshes.insert(key, GpuMesh { allocation, centers, used: self.epoch });
        self.stats.uploaded_bytes += bytes;
        *uploaded += 1;
        true
    }

    fn retire_mesh(&mut self) -> bool {
        if self.retired.len() == 4 {
            return false;
        }
        let Some(old) =
            self.meshes.iter().filter(|(_, mesh)| mesh.used != self.epoch).min_by_key(|(_, mesh)| mesh.used).map(|(key, _)| *key)
        else {
            return false;
        };
        self.retired.push(self.meshes.remove(&old).expect("selected resident"));
        true
    }
}

fn floats<const N: usize>(bytes: &mut Vec<u8>, values: [f32; N]) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
