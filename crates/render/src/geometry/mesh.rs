use super::{
    Buffer, BufferDescriptor, BufferUsages, Device, GeometryRenderer, Lease, MAX_MESH_UPLOADS, MAX_MESHES, MESH_BYTES, Ordering, Resource,
    SharedMesh, UPLOAD_BYTES, Vec3,
};

pub(super) struct GpuMesh {
    pub(super) vertices: Buffer,
    pub(super) indices: Buffer,
    pub(super) centers: Vec<Vec3>,
    pub(super) used: u64,
    _lease: Lease,
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
            .checked_mul(32)
            .and_then(|vertices| mesh.indices().len().checked_mul(4).and_then(|indices| vertices.checked_add(indices)))
        else {
            return false;
        };
        if *uploaded >= MAX_MESH_UPLOADS || bytes > UPLOAD_BYTES.saturating_sub(self.stats.uploaded_bytes) {
            return false;
        }
        if self.retired.len() < 4
            && (self.meshes.len() >= MAX_MESHES || self.memory.bytes.load(Ordering::Acquire).saturating_add(bytes) > MESH_BYTES)
            && let Some(old) = self.meshes.iter().filter(|(_, m)| m.used != self.epoch).min_by_key(|(_, m)| m.used).map(|(key, _)| *key)
        {
            self.retired.push(self.meshes.remove(&old).expect("selected resident"));
        }
        let Some(lease) = self.memory.reserve(bytes, Resource::Mesh) else { return false };
        if self.meshes.len() >= MAX_MESHES {
            return false;
        }
        let vertices = buffer(device, "Scene vertices", mesh.vertices().len() * 32, BufferUsages::VERTEX);
        let indices = buffer(device, "Scene indices", mesh.indices().len() * 4, BufferUsages::INDEX);
        let mut packed = Vec::with_capacity(bytes);
        for vertex in mesh.vertices() {
            floats(&mut packed, vertex.position.to_array());
            floats(&mut packed, vertex.normal.to_array());
            floats(&mut packed, vertex.uv.to_array());
        }
        self.queue.write_buffer(&vertices, 0, &packed);
        packed.clear();
        for index in mesh.indices() {
            packed.extend_from_slice(&index.to_le_bytes());
        }
        self.queue.write_buffer(&indices, 0, &packed);
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
        self.meshes.insert(key, GpuMesh { vertices, indices, centers, used: self.epoch, _lease: lease });
        self.stats.uploaded_bytes += bytes;
        *uploaded += 1;
        true
    }
}

pub(super) fn buffer(device: &Device, label: &str, bytes: usize, usage: BufferUsages) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: bytes as u64,
        usage: usage | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn floats<const N: usize>(bytes: &mut Vec<u8>, values: [f32; N]) {
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}
