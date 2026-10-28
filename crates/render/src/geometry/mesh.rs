use wgpu::Device;

use super::{GeometryRenderer, MAX_MESH_UPLOADS, UPLOAD_BYTES};
use crate::geometry::storage::Allocation;
use crate::pipeline::VERTEX_BYTES;
use crate::scene::SharedMesh;

pub(super) struct GpuMesh {
    pub(super) allocation: Allocation,
    pub(super) used: u64,
}

impl GeometryRenderer {
    pub(super) fn upload_mesh(&mut self, device: &Device, shared: &SharedMesh, composition: Option<&[f32]>, uploaded: &mut usize) -> bool {
        let key = shared.id();
        if let Some(mesh) = self.meshes.get_mut(&key) {
            mesh.used = self.epoch;
            self.mesh_budget_blocked.remove(&key);
            return true;
        }
        let mesh = shared.mesh();
        if composition.is_some_and(|values| values.len() != mesh.vertices().len()) {
            return false;
        }
        let Some(bytes) = mesh
            .vertices()
            .len()
            .checked_mul(VERTEX_BYTES)
            .and_then(|vertices| mesh.indices().len().checked_mul(4).and_then(|indices| vertices.checked_add(indices)))
            .and_then(|bytes| composition.map_or(Some(bytes), |values| values.len().checked_mul(size_of::<f32>())?.checked_add(bytes)))
        else {
            return false;
        };
        if *uploaded >= MAX_MESH_UPLOADS || bytes > UPLOAD_BYTES.saturating_sub(self.stats.uploaded_bytes) {
            return false;
        }
        let Some(allocation) =
            self.storage.allocate(device, mesh.vertices().len(), mesh.indices().len(), composition.is_some(), &self.memory)
        else {
            if !self.retire_mesh() {
                self.mesh_budget_blocked.insert(key);
            }
            return false;
        };
        let mut packed = Vec::with_capacity(bytes);
        for vertex in mesh.vertices() {
            floats(&mut packed, vertex.position.to_array());
            floats(&mut packed, vertex.normal.to_array());
            floats(&mut packed, vertex.uv.to_array());
        }
        self.queue.write_buffer(&allocation.buffers().vertices, u64::from(allocation.vertices().start) * VERTEX_BYTES as u64, &packed);
        if let Some(composition) = composition {
            let mut packed = Vec::with_capacity(std::mem::size_of_val(composition));
            floats_slice(&mut packed, composition);
            let buffer = allocation.buffers().composition.as_ref().expect("composition allocation has a terrain buffer");
            self.queue.write_buffer(buffer, u64::from(allocation.vertices().start) * size_of::<f32>() as u64, &packed);
        }
        packed.clear();
        for index in mesh.indices() {
            packed.extend_from_slice(&index.to_le_bytes());
        }
        self.queue.write_buffer(&allocation.buffers().indices, u64::from(allocation.indices().start) * size_of::<u32>() as u64, &packed);
        self.meshes.insert(key, GpuMesh { allocation, used: self.epoch });
        self.mesh_budget_blocked.remove(&key);
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

fn floats_slice(bytes: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
#[path = "mesh_tests.rs"]
mod tests;
