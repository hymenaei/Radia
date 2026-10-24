use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::studio::{INSTANCE_BYTES, Pipeline, ScenePipelines};
use crate::water::{COLOR, Ocean};
use crate::{
    appearance::FaceParams,
    texture::{TextureCompletion, TextureJob, TextureKey, TextureState, Textures},
};
use assets::{Priority, TextureId};
use math::{DVec3, Frustum, Mat4, Quat, Vec3, flips_winding, normal_matrix};
use scene::{Camera, FaceMaterial, MeshId, Placement, PreparedMesh, SceneView, SharedMesh, SurfaceKind, ViewVersion};
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, Extent3d, IndexFormat, Queue, RenderPass, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;

const MESH_BYTES: usize = 64 * 1024 * 1024;
const DEPTH_BYTES: usize = 128 * 1024 * 1024;
const UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_MESH_UPLOADS: usize = 16;
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
struct Lease {
    memory: Arc<Accounting>,
    bytes: usize,
    resource: Resource,
}
enum Resource {
    Mesh,
    Depth,
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
impl Drop for Lease {
    fn drop(&mut self) {
        let account = match self.resource {
            Resource::Mesh => &self.memory.bytes,
            Resource::Depth => &self.memory.depth,
        };
        account.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub(crate) struct FrameLease(Arc<Accounting>);
impl Drop for FrameLease {
    fn drop(&mut self) {
        self.0.frames.fetch_sub(1, Ordering::AcqRel);
    }
}

struct GpuMesh {
    vertices: Buffer,
    indices: Buffer,
    centers: Vec<Vec3>,
    used: u64,
    _lease: Lease,
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

struct Group {
    first: (u8, u128, u16, u8),
    bytes: Vec<u8>,
    count: u32,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct DrawKey {
    mesh: MeshId,
    start: u32,
    end: u32,
    pipeline: Pipeline,
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
    ocean: Ocean,
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
            ocean: Ocean::new(device, format),
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

    pub(crate) fn prepare(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        view: &SceneView,
        camera: Camera,
        size: [u32; 2],
        water_height: Option<f64>,
    ) {
        let started = Instant::now();
        let mut times = PrepareTimes::default();
        let key = FramePreparationKey { version: view.version(), camera, size, water_height };
        let same = self.prepared.as_ref().is_some_and(|prepared| prepared.key == key);
        if same {
            let upload_started = Instant::now();
            self.textures.upload(device, encoder, self.epoch);
            times.textures = upload_started.elapsed();
            if self
                .prepared
                .as_ref()
                .is_some_and(|prepared| prepared.reusable(&key, self.textures.state(), self.memory.bytes.load(Ordering::Acquire)))
            {
                self.stats.uploaded_bytes = 0;
                self.memory_stats();
                self.record_prepare(started, times, 0);
                return;
            }
        }
        let visibility_started = Instant::now();
        self.epoch = self.epoch.checked_add(1).expect("render frame space exhausted");
        self.stats = Stats::default();
        self.draws.clear();
        self.visible.clear();
        let projection = self.studio.update_camera_uniform(&self.queue, camera, size);
        self.ocean.update(&self.queue, view, camera, projection, water_height);
        let objects = view.objects().filter_map(|object| {
            let (Some((_, mesh)), Placement::Ready(transform)) = (object.mesh(), object.placement()) else { return None };
            Some((
                (2, object.detail().summary.id.as_u128(), 0),
                Instance {
                    mesh: mesh.clone(),
                    position: transform.pose.position,
                    rotation: transform.pose.rotation,
                    scale: transform.scale,
                    pickable: true,
                    faces: Vec::new(),
                    shading: ShadingMode::Studio,
                },
                object.materials(),
            ))
        });
        let surfaces = view.surfaces().filter_map(|surface| {
            let PreparedMesh::Ready(mesh) = &surface.mesh else { return None };
            let (kind, index, shading) = match surface.kind {
                SurfaceKind::Ground(patch) => {
                    let (x, y) = patch.coordinates();
                    (1, u16::from(y) * 16 + u16::from(x), ShadingMode::Ground)
                }
                SurfaceKind::Water => (0, 0, ShadingMode::Water),
            };
            Some((
                (kind, u128::from(surface.region.get()), index),
                Instance {
                    mesh: mesh.clone(),
                    position: surface.position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    pickable: surface.kind != SurfaceKind::Water,
                    faces: Vec::new(),
                    shading,
                },
                None,
            ))
        });
        let mut candidates = Vec::new();
        let frustum = Frustum::from_clip_matrix(projection);
        for candidate in surfaces.chain(objects) {
            if is_visible(&candidate.1, camera, &frustum) {
                candidates.push(candidate);
            }
            if candidates.len() == MAX_INSTANCES {
                break;
            }
        }
        candidates.sort_unstable_by_key(|(id, _, _)| *id);
        times.visibility = visibility_started.elapsed();
        let requests_started = Instant::now();
        self.requested_textures.clear();
        let mut request = |face: &FaceMaterial, priority| {
            let Some(texture) = &face.base_color else { return };
            if self.requested_textures.contains(&texture.id()) {
                return;
            }
            if priority == Priority::Background && !texture.info().is_some_and(|info| info.size.into_iter().all(|edge| edge <= 32)) {
                self.requested_textures.insert(texture.id());
                return;
            }
            if FaceParams::new(Some(face)).texture.is_none() {
                return;
            }
            self.requested_textures.insert(texture.id());
            self.textures.request(texture, self.epoch, priority);
        };
        for (_, instance, faces) in &candidates {
            if let Some(mesh) = self.meshes.get_mut(&instance.mesh.id()) {
                mesh.used = self.epoch;
            }
            if let Some(faces) = faces {
                for face in *faces {
                    request(face, Priority::Visible);
                }
            }
        }
        for object in view.objects() {
            if let Some(faces) = object.materials() {
                for face in faces {
                    request(face, Priority::Background);
                }
            }
        }
        times.requests = requests_started.elapsed();
        let upload_started = Instant::now();
        if !same {
            self.textures.upload(device, encoder, self.epoch);
        }
        times.textures += upload_started.elapsed();
        let faces_started = Instant::now();
        let mut groups: HashMap<DrawKey, Group> = HashMap::new();
        let mut blended = Vec::new();
        let mut uploaded = 0;
        let mut count = 0;
        let mut pending_meshes = HashSet::new();
        let mut pending_textures = HashSet::new();
        for (id, mut instance, faces) in candidates {
            if !self.upload_mesh(device, &instance.mesh, &mut uploaded) {
                pending_meshes.insert(instance.mesh.id());
                continue;
            }
            let base = instance_base(&instance, camera.position());
            for (index, face) in instance.mesh.mesh().faces().iter().enumerate() {
                if face.indices.is_empty() {
                    continue;
                }
                let mut params = FaceParams::new(faces.and_then(|faces| faces.iter().find(|material| material.slot == face.slot)));
                match instance.shading {
                    ShadingMode::Ground => {
                        params.color = [0.18, 0.23, 0.13, 1.0];
                        params.params[0] = 1.0;
                    }
                    ShadingMode::Water => {
                        params.color = COLOR.to_array();
                        params.params[0] = 1.0;
                    }
                    ShadingMode::Studio => {}
                }
                if params.blend && params.color[3] <= 0.0 {
                    continue;
                }
                let texture = params.texture.and_then(|key| self.textures.resident(device, key));
                if texture.is_none()
                    && let Some(key) = params.texture
                {
                    pending_textures.insert(key.id);
                }
                let pipeline = if matches!(instance.shading, ShadingMode::Water) {
                    Pipeline::Water
                } else {
                    match (params.blend, params.double_sided, flips_winding(instance.scale)) {
                        (true, true, _) => Pipeline::BlendDoubleSided,
                        (true, false, true) => Pipeline::BlendFlipped,
                        (true, false, false) => Pipeline::Blend,
                        (false, true, _) => Pipeline::DoubleSided,
                        (false, false, true) => Pipeline::FlippedWinding,
                        (false, false, false) => Pipeline::Studio,
                    }
                };
                let key = DrawKey { mesh: instance.mesh.id(), start: face.indices.start, end: face.indices.end, pipeline, texture };
                if count == MAX_FACE_INSTANCES
                    || (groups.len() + blended.len() >= MAX_DRAWS && (params.blend || !groups.contains_key(&key)))
                {
                    self.stats.omitted_faces += 1;
                    continue;
                }
                count += 1;
                let bytes = packed_instance(&base, &params, instance.scale);
                let order = (id.0, id.1, id.2, face.slot);
                if params.blend {
                    let center = self.meshes[&key.mesh].centers[index];
                    let relative = instance.position - camera.position() + (instance.rotation * (center * instance.scale)).as_dvec3();
                    blended.push((relative.length_squared(), order, key, bytes));
                } else {
                    let group = groups.entry(key).or_insert_with(|| Group { first: order, bytes: Vec::new(), count: 0 });
                    group.first = group.first.min(order);
                    group.bytes.extend(bytes);
                    group.count += 1;
                }
                instance.faces.push(PickFace { indices: face.indices.clone(), double_sided: params.double_sided });
            }
            if !instance.faces.is_empty() {
                self.visible.push(instance);
            }
        }
        times.faces = faces_started.elapsed();
        let instances_started = Instant::now();
        let mut bytes = Vec::with_capacity(count * INSTANCE_BYTES);
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_unstable_by_key(|(_, group)| group.first);
        for (key, group) in groups {
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend(group.bytes);
            self.draws.push(DrawCommand {
                key: key.mesh,
                pipeline: key.pipeline,
                first,
                count: group.count,
                indices: key.start..key.end,
                texture: key.texture,
            });
        }
        blended.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (_, _, key, instance) in blended {
            let first = (bytes.len() / INSTANCE_BYTES) as u32;
            bytes.extend(instance);
            self.draws.push(DrawCommand {
                key: key.mesh,
                pipeline: key.pipeline,
                first,
                count: 1,
                indices: key.start..key.end,
                texture: key.texture,
            });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instances, 0, &bytes);
        }
        self.stats.instances = self.visible.len();
        self.stats.draw_calls = self.draws.len() + usize::from(self.ocean.active);
        self.memory_stats();
        self.prepared = Some(FramePreparation {
            key,
            textures: self.textures.state(),
            mesh_bytes: self.stats.mesh_bytes,
            more_meshes: !pending_meshes.is_empty() && uploaded > 0,
        });
        self.stats.pending_textures = pending_textures.len();
        self.stats.pending_meshes = pending_meshes.len();
        times.instances = instances_started.elapsed();
        self.record_prepare(started, times, uploaded);
    }

    fn record_prepare(&mut self, started: Instant, times: PrepareTimes, uploaded: usize) {
        self.stats.prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
        if self.stats.prepare_ms >= 16.0 && self.slow_report.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
            self.slow_report = Some(Instant::now());
            tracing::info!(
                total_ms = self.stats.prepare_ms,
                setup_visibility_ms = times.visibility.as_secs_f64() * 1000.0,
                texture_requests_ms = times.requests.as_secs_f64() * 1000.0,
                texture_upload_ms = times.textures.as_secs_f64() * 1000.0,
                meshes_faces_ms = times.faces.as_secs_f64() * 1000.0,
                instance_upload_ms = times.instances.as_secs_f64() * 1000.0,
                mesh_uploads = uploaded,
                mesh_upload_bytes = self.stats.uploaded_bytes,
                draws = self.stats.draw_calls,
                instances = self.stats.instances,
                pending_meshes = self.stats.pending_meshes,
                pending_textures = self.stats.pending_textures,
                "slow scene preparation"
            );
        }
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

    fn upload_mesh(&mut self, device: &Device, shared: &SharedMesh, uploaded: &mut usize) -> bool {
        let key = shared.id();
        if self.meshes.contains_key(&key) {
            return true;
        }
        let mesh = shared.mesh();
        let bytes = mesh.vertices().len() * 32 + mesh.indices().len() * 4;
        if *uploaded == MAX_MESH_UPLOADS || self.stats.uploaded_bytes + bytes > UPLOAD_BYTES {
            return false;
        }
        if self.retired.len() < 4
            && (self.meshes.len() == 1024 || self.memory.bytes.load(Ordering::Acquire) + bytes > MESH_BYTES)
            && let Some(old) = self.meshes.iter().filter(|(_, m)| m.used != self.epoch).min_by_key(|(_, m)| m.used).map(|(key, _)| *key)
        {
            self.retired.push(self.meshes.remove(&old).expect("selected resident"));
        }
        let Some(lease) = self.memory.reserve(bytes, Resource::Mesh) else { return false };
        if self.meshes.len() == 1024 {
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
        self.ocean.draw(pass);
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

fn buffer(device: &Device, label: &str, bytes: usize, usage: BufferUsages) -> Buffer {
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

fn model(instance: &Instance, origin: DVec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(instance.scale, instance.rotation, (instance.position - origin).as_vec3())
}

fn instance_base(instance: &Instance, origin: DVec3) -> [u8; TRANSFORM_BYTES] {
    let mut bytes = [0; TRANSFORM_BYTES];
    let normal = normal_matrix(instance.rotation, instance.scale);
    let values = model(instance, origin)
        .to_cols_array()
        .into_iter()
        .chain([normal.x_axis, normal.y_axis, normal.z_axis].into_iter().flat_map(|column| column.extend(0.0).to_array()));
    for (dest, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *dest = value.to_le_bytes();
    }
    bytes
}

fn packed_instance(base: &[u8; TRANSFORM_BYTES], params: &FaceParams, scale: Vec3) -> [u8; INSTANCE_BYTES] {
    let mut bytes = [0; INSTANCE_BYTES];
    bytes[..TRANSFORM_BYTES].copy_from_slice(base);
    let values = params.color.into_iter().chain(params.uv.into_iter().flatten()).chain(params.params).chain(scale.extend(0.0).to_array());
    for (dest, value) in bytes[TRANSFORM_BYTES..].as_chunks_mut::<4>().0.iter_mut().zip(values) {
        *dest = value.to_le_bytes();
    }
    bytes
}

fn is_visible(instance: &Instance, camera: Camera, frustum: &Frustum) -> bool {
    let bounds = instance.mesh.mesh().bounds();
    let center = (bounds.min.as_dvec3() + bounds.max.as_dvec3()) * 0.5 * instance.scale.as_dvec3();
    let offset = instance.position - camera.position() + (instance.rotation * center.as_vec3()).as_dvec3();
    let radius = ((bounds.max.as_dvec3() - bounds.min.as_dvec3()) * 0.5 * instance.scale.as_dvec3()).length();
    if !offset.is_finite()
        || !radius.is_finite()
        || offset.length() - radius > camera.draw_distance()
        || offset.abs().max_element() + radius > f64::from(f32::MAX) / 16.0
    {
        return false;
    }
    let model = model(instance, camera.position());
    if !model.is_finite() || !instance.scale.recip().is_finite() {
        return false;
    }
    frustum.intersects_sphere(offset, radius)
}
