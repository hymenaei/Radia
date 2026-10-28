use math::Vec3;
use wgpu::Device;

use assets::mesh::MeshLod;
use utils::AssetId;

use super::{GeometryRenderer, MAX_MESH_UPLOADS, MAX_MESHES, UPLOAD_BYTES};
use crate::geometry::storage::Allocation;
use crate::pipeline::VERTEX_BYTES;
use crate::scene::SharedMesh;

pub(super) struct GpuMesh {
    pub(super) allocation: Allocation,
    pub(super) centers: Vec<Vec3>,
    pub(super) used: u64,
    pub(super) asset_lod: Option<(AssetId, MeshLod)>,
    pub(super) shared: Option<SharedMesh>,
}

impl GeometryRenderer {
    pub(super) fn upload_mesh(&mut self, device: &Device, shared: &SharedMesh, composition: Option<&[f32]>, uploaded: &mut usize) -> bool {
        let key = shared.id();
        if self.meshes.contains_key(&key) {
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
        if !within_upload_target(bytes, *uploaded, self.stats.uploaded_bytes) {
            return false;
        }
        if self.meshes.len() >= MAX_MESHES && !self.retire_mesh() {
            return false;
        }
        let Some(allocation) =
            self.storage.allocate(device, mesh.vertices().len(), mesh.indices().len(), composition.is_some(), &self.memory)
        else {
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
        let asset_lod = shared.asset_lod();
        self.meshes.insert(key, GpuMesh { allocation, centers, used: self.epoch, asset_lod, shared: asset_lod.map(|_| shared.clone()) });
        if let Some(asset_lod) = asset_lod {
            self.asset_meshes.insert(asset_lod, key);
        }
        self.stats.uploaded_bytes += bytes;
        *uploaded += 1;
        true
    }

    fn retire_mesh(&mut self) -> bool {
        if self.retired.len() == 4 {
            return false;
        }
        let Some(old) = retirement_candidate(self.meshes.iter().map(|(key, mesh)| (*key, mesh.used)), self.epoch) else {
            return false;
        };
        let mesh = self.meshes.remove(&old).expect("selected resident");
        if let Some(asset_lod) = mesh.asset_lod
            && self.asset_meshes.get(&asset_lod) == Some(&old)
        {
            self.asset_meshes.remove(&asset_lod);
        }
        self.retired.push(mesh);
        true
    }
}

fn retirement_candidate<K: Copy>(meshes: impl Iterator<Item = (K, u64)>, epoch: u64) -> Option<K> {
    meshes.filter(|(_, used)| *used != epoch).min_by_key(|(_, used)| *used).map(|(key, _)| key)
}

pub(super) fn within_upload_target(bytes: usize, uploaded: usize, uploaded_bytes: usize) -> bool {
    uploaded < MAX_MESH_UPLOADS && (uploaded == 0 || uploaded_bytes.checked_add(bytes).is_some_and(|total| total <= UPLOAD_BYTES))
}

pub(super) fn resident_mesh_fallback<K: Copy + Eq>(
    meshes: &std::collections::HashMap<(AssetId, MeshLod), K>,
    asset: AssetId,
    desired: MeshLod,
    selected: K,
) -> Option<K> {
    [MeshLod::Lowest, MeshLod::Low, MeshLod::Medium, MeshLod::High]
        .into_iter()
        .filter_map(|lod| meshes.get(&(asset, lod)).copied().map(|id| (id, lod)))
        .filter(|(id, _)| *id != selected)
        .min_by_key(|(_, lod)| {
            let index = lod_index(*lod);
            (index.abs_diff(lod_index(desired)), usize::from(index > lod_index(desired)))
        })
        .map(|(id, _)| id)
}

fn lod_index(lod: MeshLod) -> usize {
    match lod {
        MeshLod::Lowest => 0,
        MeshLod::Low => 1,
        MeshLod::Medium => 2,
        MeshLod::High => 3,
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
