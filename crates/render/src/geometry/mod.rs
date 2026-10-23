mod frame;
mod instance;
mod mesh;

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use assets::{Priority, TextureId};
use math::{DVec3, Frustum, Mat4, Quat, Vec3};
use scene::{Camera, FaceMaterial, MeshId, Placement, PreparedMesh, SceneView, SharedMesh, SurfaceKind, ViewVersion};
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, Extent3d, IndexFormat, Queue, RenderPass, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};

use crate::{
    appearance::FaceParams,
    studio::{INSTANCE_BYTES, Pipeline, ScenePipelines},
    texture::{TextureCompletion, TextureJob, TextureKey, TextureState, Textures},
    water::{COLOR, Water},
};
use instance::{instance_base, is_visible, packed_instance};
use mesh::{GpuMesh, buffer};

const MESH_BYTES: usize = 64 * 1024 * 1024;
const DEPTH_BYTES: usize = 128 * 1024 * 1024;
const UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_MESH_UPLOADS: usize = 16;
const MAX_MESHES: usize = 1024;
const MAX_INSTANCES: usize = 16_384;
const MAX_FACE_INSTANCES: usize = 65_536;
const MAX_DRAWS: usize = 8192;
const TRANSFORM_BYTES: usize = 112;

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub instances: usize,
    pub draw_calls: usize,
    pub pending_meshes: usize,
    pub uploaded_bytes: usize,
    pub mesh_bytes: usize,
    pub mesh_peak: usize,
    pub depth_bytes: usize,
    pub prepare_ms: f64,
    pub texture_bytes: usize,
    pub texture_count: usize,
    pub texture_failure: Option<assets::Failure>,
    pub staging_bytes: usize,
    pub texture_upload_bytes: usize,
    pub pending_textures: usize,
    pub omitted_faces: usize,
}

#[derive(Default)]
struct Accounting {
    bytes: AtomicUsize,
    depth: AtomicUsize,
    peak: AtomicUsize,
    frames: AtomicUsize,
}

impl Accounting {
    fn reserve(self: &Arc<Self>, bytes: usize, resource: Resource) -> Option<Lease> {
        let (account, limit) = match resource {
            Resource::Mesh => (&self.bytes, MESH_BYTES),
            Resource::Depth => (&self.depth, DEPTH_BYTES),
        };
        let before =
            account.try_update(Ordering::AcqRel, Ordering::Acquire, |used| used.checked_add(bytes).filter(|n| *n <= limit)).ok()?;
        if matches!(resource, Resource::Mesh) {
            self.peak.fetch_max(before + bytes, Ordering::AcqRel);
        }
        Some(Lease { memory: self.clone(), bytes, resource })
    }
}

struct Lease {
    memory: Arc<Accounting>,
    bytes: usize,
    resource: Resource,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let account = match self.resource {
            Resource::Mesh => &self.memory.bytes,
            Resource::Depth => &self.memory.depth,
        };
        account.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

enum Resource {
    Mesh,
    Depth,
}

pub(crate) struct FrameLease(Arc<Accounting>);

impl Drop for FrameLease {
    fn drop(&mut self) {
        self.0.frames.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct Instance {
    pub mesh: SharedMesh,
    pub position: DVec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub pickable: bool,
    pub faces: Vec<PickFace>,
    shading: ShadingMode,
}

pub struct PickFace {
    pub indices: Range<u32>,
    pub double_sided: bool,
}

enum ShadingMode {
    Studio,
    Ground,
    Water,
}

struct DrawCommand {
    key: MeshId,
    pipeline: Pipeline,
    first: u32,
    count: u32,
    indices: Range<u32>,
    texture: Option<TextureKey>,
}

struct Depth {
    _texture: Texture,
    view: TextureView,
    size: [u32; 2],
    _lease: Lease,
}

#[derive(PartialEq)]
struct FramePreparationKey {
    version: ViewVersion,
    camera: Camera,
    size: [u32; 2],
    water_height: Option<f64>,
}

struct FramePreparation {
    key: FramePreparationKey,
    textures: TextureState,
    mesh_bytes: usize,
    more_meshes: bool,
}

impl FramePreparation {
    fn reusable(&self, key: &FramePreparationKey, textures: TextureState, mesh_bytes: usize) -> bool {
        self.key == *key && self.textures == textures && self.mesh_bytes == mesh_bytes && !self.more_meshes
    }
}

#[derive(Default)]
struct PrepareTimes {
    visibility: Duration,
    requests: Duration,
    textures: Duration,
    faces: Duration,
    instances: Duration,
}

pub(crate) struct GeometryRenderer {
    queue: Queue,
    memory: Arc<Accounting>,
    studio: ScenePipelines,
    water: Water,
    textures: Textures,
    requested_textures: HashSet<TextureId>,
    instances: Buffer,
    depth: Option<Depth>,
    meshes: HashMap<MeshId, GpuMesh>,
    retired: Vec<GpuMesh>,
    draws: Vec<DrawCommand>,
    visible: Vec<Instance>,
    epoch: u64,
    prepared: Option<FramePreparation>,
    slow_report: Option<Instant>,
    pub(crate) stats: Stats,
}

impl GeometryRenderer {
    pub(crate) fn release_textures(&mut self) {
        self.textures.release_resources();
    }

    pub(crate) fn needs_frame(&self) -> bool {
        self.prepared.as_ref().is_some_and(|prepared| {
            prepared.more_meshes
                || prepared.textures != self.textures.state()
                || prepared.mesh_bytes != self.memory.bytes.load(Ordering::Acquire)
        }) || self.textures.uploading()
    }
    pub(crate) fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self {
        static MEMORY: OnceLock<Arc<Accounting>> = OnceLock::new();
        let textures = Textures::new(device, queue);
        let studio = ScenePipelines::new(device, format, &textures.layout);
        Self {
            queue: queue.clone(),
            memory: MEMORY.get_or_init(|| Arc::new(Accounting::default())).clone(),
            studio,
            textures,
            requested_textures: HashSet::new(),
            water: Water::new(device, format),
            instances: buffer(device, "Face instances", MAX_FACE_INSTANCES * INSTANCE_BYTES, BufferUsages::VERTEX),
            depth: None,
            meshes: HashMap::new(),
            retired: Vec::new(),
            draws: Vec::new(),
            visible: Vec::new(),
            epoch: 0,
            prepared: None,
            slow_report: None,
            stats: Stats::default(),
        }
    }

    pub(crate) fn admit(&self) -> Option<FrameLease> {
        self.memory.frames.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 3).then_some(n + 1)).ok()?;
        Some(FrameLease(self.memory.clone()))
    }

    pub(crate) fn depth(&mut self, device: &Device, size: [u32; 2]) -> Option<TextureView> {
        if self.depth.as_ref().is_some_and(|depth| depth.size != size) {
            let old = self.depth.take();
            self.queue.on_submitted_work_done(move || drop(old));
        }
        if self.depth.is_none() {
            let bytes = (size[0] as usize).checked_mul(size[1] as usize)?.checked_mul(4)?;
            let lease = self.memory.reserve(bytes, Resource::Depth)?;
            let texture = device.create_texture(&TextureDescriptor {
                label: Some("Scene depth"),
                size: Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: TextureFormat::Depth32Float,
                usage: TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&TextureViewDescriptor::default());
            self.depth = Some(Depth { _texture: texture, view, size, _lease: lease });
        }
        self.stats.depth_bytes = self.memory.depth.load(Ordering::Acquire);
        Some(self.depth.as_ref().expect("depth initialized").view.clone())
    }

    pub(crate) fn skip(&mut self) {
        self.prepared = None;
        self.visible.clear();
        self.draws.clear();
        self.stats.instances = 0;
        self.stats.draw_calls = 0;
    }

    fn memory_stats(&mut self) {
        self.stats.depth_bytes = self.memory.depth.load(Ordering::Acquire);
        self.stats.mesh_bytes = self.memory.bytes.load(Ordering::Acquire);
        self.stats.mesh_peak = self.memory.peak.load(Ordering::Acquire);
        self.stats.texture_bytes = self.textures.bytes();
        self.stats.texture_count = self.textures.count();
        self.stats.texture_failure = self.textures.failure();
        self.stats.staging_bytes = self.textures.staging_bytes();
        self.stats.texture_upload_bytes = self.textures.uploaded;
    }

    pub(crate) fn next_texture_job(&mut self) -> Option<TextureJob> {
        self.textures.next_conversion_job()
    }
    pub(crate) fn complete_texture(&mut self, completion: TextureCompletion) {
        self.textures.complete(completion);
    }
    pub(crate) fn texture_soft_cap_mib(&self) -> u32 {
        self.textures.soft_cap_mib()
    }
    pub(crate) fn set_texture_soft_cap(&mut self, mib: u32) {
        self.textures.set_soft_cap(mib);
    }
    pub(crate) fn draw(&self, pass: &mut RenderPass<'_>) {
        self.water.draw(pass);
        self.draw_meshes(pass, &self.draws);
    }

    fn draw_meshes(&self, pass: &mut RenderPass<'_>, draws: &[DrawCommand]) {
        pass.set_vertex_buffer(1, self.instances.slice(..));
        for draw in draws {
            self.studio.bind_pipeline(pass, draw.pipeline);
            pass.set_bind_group(1, self.textures.binding(draw.texture), &[]);
            let mesh = &self.meshes[&draw.key];
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), IndexFormat::Uint32);
            pass.draw_indexed(draw.indices.clone(), 0, draw.first..draw.first + draw.count);
        }
    }

    pub(crate) fn submitted(&mut self, frame: FrameLease) {
        self.textures.submitted(&self.queue);
        let retired = std::mem::take(&mut self.retired);
        self.queue.on_submitted_work_done(move || {
            drop(retired);
            drop(frame);
        });
    }

    pub(crate) fn instances(&self) -> &[Instance] {
        &self.visible
    }
}

impl Drop for GeometryRenderer {
    fn drop(&mut self) {
        let mut meshes: Vec<_> = self.meshes.drain().map(|(_, mesh)| mesh).collect();
        meshes.append(&mut self.retired);
        let depth = self.depth.take();
        self.queue.on_submitted_work_done(move || {
            drop(meshes);
            drop(depth);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_admission() {
        let memory = Arc::new(Accounting::default());
        let a = memory.reserve(MESH_BYTES, Resource::Mesh).unwrap();
        assert!(memory.reserve(1, Resource::Mesh).is_none());
        let b = memory.reserve(DEPTH_BYTES, Resource::Depth).unwrap();
        assert!(memory.reserve(1, Resource::Depth).is_none());
        drop(a);
        drop(b);
        assert_eq!(memory.bytes.load(Ordering::Acquire), 0);
        assert_eq!(memory.depth.load(Ordering::Acquire), 0);
    }
}
