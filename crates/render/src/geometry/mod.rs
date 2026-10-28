mod batching;
mod cache;
mod frame;
mod instance;
mod mesh;
mod pick;
mod storage;

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use assets::TextureId;
use math::{DVec3, Quat, Vec3};
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, Device, Extent3d, Queue, RenderPass, Texture, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};

use self::batching::{Batches, DrawCommand, INDIRECT_DRAW_BYTES, MAX_FACE_INSTANCES};
use self::cache::FrameCache;
use self::mesh::GpuMesh;
use self::storage::Storage;
use crate::scene::{Camera, MeshId, SharedMesh, ViewVersion};
use crate::{
    pipeline::{SurfacePipelines, Water},
    texture::{TextureCompletion, TextureJob, TextureState, Textures},
};

pub use pick::PickError;

const MESH_BYTES: usize = 64 * 1024 * 1024;
const DEPTH_BYTES: usize = 128 * 1024 * 1024;
pub(super) const INSTANCE_BUFFER_BYTES: usize = 16 * 1024 * 1024;
const UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_MESH_UPLOADS: usize = 16;
const TRANSFORM_BYTES: usize = 112;

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub instances: usize,
    pub draw_calls: usize,
    pub draw_commands: usize,
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
    pub mesh_budget_blocked: usize,
    pub omission_reason: Option<OmissionReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OmissionReason {
    InstanceBufferBudget,
    MeshMemoryBudget,
    CombinedBudgets,
}

#[derive(Default)]
struct Accounting {
    bytes: AtomicUsize,
    mesh_revision: AtomicU64,
    depth: AtomicUsize,
    peak: AtomicUsize,
    frames: AtomicUsize,
}

impl Accounting {
    fn mesh_state(&self) -> MeshState {
        MeshState { bytes: self.bytes.load(Ordering::Acquire), revision: self.mesh_revision.load(Ordering::Acquire) }
    }

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

#[derive(Clone, Copy, Default, Eq, PartialEq)]
struct MeshState {
    bytes: usize,
    revision: u64,
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

pub(crate) struct Instance {
    mesh: SharedMesh,
    position: DVec3,
    rotation: Quat,
    scale: Vec3,
    pickable: bool,
    faces: Vec<PickFace>,
}

pub(crate) struct PickFace {
    indices: Range<u32>,
    double_sided: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ShadingMode {
    Studio,
    Terrain,
    Water,
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
    meshes: MeshState,
    more_meshes: bool,
    animated_faces: bool,
}

impl FramePreparation {
    fn needs_frame(&self, textures: TextureState, meshes: MeshState) -> bool {
        self.animated_faces || self.more_meshes || self.textures != textures || self.meshes != meshes
    }

    fn reusable(&self, key: &FramePreparationKey, textures: TextureState, meshes: MeshState) -> bool {
        self.key == *key && self.textures == textures && self.meshes == meshes && !self.more_meshes && !self.animated_faces
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
    pipelines: SurfacePipelines,
    water: Water,
    textures: Textures,
    requested_textures: HashSet<TextureId>,
    instances: Buffer,
    indirect: Buffer,
    indirect_first_instance: bool,
    depth: Option<Depth>,
    meshes: HashMap<MeshId, GpuMesh>,
    storage: Storage,
    retired: Vec<GpuMesh>,
    cache: FrameCache,
    batches: Batches,
    pending_meshes: HashSet<MeshId>,
    pending_textures: HashSet<TextureId>,
    draws: Vec<DrawCommand>,
    draw_report: Option<Instant>,
    omitted_faces: usize,
    mesh_budget_blocked: HashSet<MeshId>,
    visible: Vec<Instance>,
    visible_count: usize,
    instance_bytes: Vec<u8>,
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
        self.prepared.as_ref().is_some_and(|prepared| prepared.needs_frame(self.textures.state(), self.memory.mesh_state()))
            || self.textures.uploading()
    }
    pub(crate) fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self {
        static MEMORY: OnceLock<Arc<Accounting>> = OnceLock::new();
        let textures = Textures::new(device, queue);
        let pipelines = SurfacePipelines::new(device, format, &textures.layout, &textures.terrain_layout);
        let indirect_bytes = MAX_FACE_INSTANCES * INDIRECT_DRAW_BYTES;
        Self {
            queue: queue.clone(),
            memory: MEMORY.get_or_init(|| Arc::new(Accounting::default())).clone(),
            pipelines,
            textures,
            requested_textures: HashSet::new(),
            water: Water::new(device, format),
            instances: buffer(device, "Face instances", INSTANCE_BUFFER_BYTES, BufferUsages::VERTEX),
            indirect: buffer(device, "Scene indexed draws", indirect_bytes, BufferUsages::INDIRECT),
            indirect_first_instance: device.features().contains(wgpu::Features::INDIRECT_FIRST_INSTANCE),
            depth: None,
            meshes: HashMap::new(),
            storage: Storage::default(),
            retired: Vec::new(),
            cache: FrameCache::default(),
            batches: Batches::default(),
            pending_meshes: HashSet::new(),
            pending_textures: HashSet::new(),
            draws: Vec::new(),
            draw_report: None,
            omitted_faces: 0,
            mesh_budget_blocked: HashSet::new(),
            visible: Vec::new(),
            visible_count: 0,
            instance_bytes: Vec::new(),
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
        self.visible_count = 0;
        self.draws.clear();
        self.batches.clear();
        self.stats.instances = 0;
        self.stats.draw_calls = 0;
        self.stats.draw_commands = 0;
        self.omitted_faces = 0;
        self.mesh_budget_blocked.clear();
        self.stats.omitted_faces = 0;
        self.stats.mesh_budget_blocked = 0;
        self.stats.omission_reason = None;
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
        let mut pipeline = None;
        let mut texture = None;
        let mut terrain = None;
        let mut buffers = None;
        for draw in self.batches.draw_batches() {
            if pipeline != Some(draw.key.pipeline) {
                self.pipelines.bind_pipeline(pass, draw.key.pipeline);
                pipeline = Some(draw.key.pipeline);
            }
            if texture != Some(draw.key.texture) {
                pass.set_bind_group(1, self.textures.binding(draw.key.texture), &[]);
                texture = Some(draw.key.texture);
            }
            if draw.key.pipeline == crate::pipeline::Pipeline::Terrain {
                let key = draw.key.terrain.expect("terrain batch has detail texture bindings");
                if terrain != Some(key) {
                    pass.set_bind_group(2, self.textures.terrain_binding(key), &[]);
                    terrain = Some(key);
                }
            }
            let page = Arc::as_ptr(&draw.buffers) as usize;
            if buffers != Some(page) {
                pass.set_vertex_buffer(0, draw.buffers.vertices.slice(..));
                pass.set_index_buffer(draw.buffers.indices.slice(..), wgpu::IndexFormat::Uint32);
                buffers = Some(page);
            }
            let instance_slot = if draw.key.pipeline == crate::pipeline::Pipeline::Terrain { 2 } else { 1 };
            if draw.key.pipeline == crate::pipeline::Pipeline::Terrain {
                let composition = draw.buffers.composition.as_ref().expect("terrain buffer has composition attributes");
                pass.set_vertex_buffer(1, composition.slice(..));
            }
            if self.indirect_first_instance {
                pass.set_vertex_buffer(instance_slot, self.instances.slice(..));
                pass.multi_draw_indexed_indirect(&self.indirect, draw.offset, draw.count);
            } else {
                for command in 0..draw.count {
                    let first_instance = self.batches.first_instance(draw.offset, command);
                    let start = u64::from(first_instance) * crate::pipeline::INSTANCE_BYTES as u64;
                    pass.set_vertex_buffer(instance_slot, self.instances.slice(start..));
                    pass.draw_indexed_indirect(&self.indirect, draw.offset + u64::from(command) * INDIRECT_DRAW_BYTES as u64);
                }
            }
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

fn buffer(device: &Device, label: &str, bytes: usize, usage: BufferUsages) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: bytes as u64,
        usage: usage | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod fixtures;

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
