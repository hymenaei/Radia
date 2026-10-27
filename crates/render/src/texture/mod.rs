mod compression;
mod job;
mod upload;

pub use compression::CompressionQuality;

use std::{
    collections::{HashMap, VecDeque},
    mem::{replace, take},
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use assets::{
    Failure, Filter, Image, MinFilter, Priority, Sampler, TextureDecodeJob, TextureEvents, TextureHandle, TextureId, WeakTexture, Wrap,
};
use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry,
    BindingResource, BindingType, Buffer, BufferDescriptor, BufferUsages, COPY_BYTES_PER_ROW_ALIGNMENT, Color, ColorTargetState,
    ColorWrites, CommandEncoder, Device, Extent3d, FilterMode, FragmentState, LoadOp, MipmapFilterMode, Operations, Origin3d,
    PipelineLayoutDescriptor, Queue, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor,
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureView, TextureViewDescriptor, TextureViewDimension, VertexState,
};

const DEFAULT_SOFT_CAP: usize = 1024 * 1024 * 1024;
const STAGING_BYTES: usize = 48 * 1024 * 1024;
const UPLOAD_BYTES: usize = 4 * 1024 * 1024;
const MAX_JOBS: usize = 2;
const MIP_VIEW_FORMATS: [TextureFormat; 1] = [TextureFormat::Rgba8Unorm];

#[derive(Default)]
struct Accounting {
    gpu: AtomicUsize,
    textures: AtomicUsize,
    staging: AtomicUsize,
    jobs: AtomicUsize,
}

impl Accounting {
    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Lease> {
        self.staging
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| used.checked_add(bytes).filter(|total| *total <= STAGING_BYTES))
            .ok()?;
        Some(Lease { memory: self.clone(), bytes })
    }

    fn work(self: &Arc<Self>) -> Option<Work> {
        self.jobs.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_JOBS).then_some(n + 1)).ok()?;
        Some(Work(self.clone()))
    }
}

struct Lease {
    memory: Arc<Accounting>,
    bytes: usize,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.memory.staging.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct GpuCharge {
    stats: Arc<Accounting>,
    cap: Arc<AtomicUsize>,
    bytes: usize,
}

impl GpuCharge {
    fn new(stats: Arc<Accounting>, cap: Arc<AtomicUsize>, descriptor: &TextureDescriptor<'_>) -> Self {
        let bytes = allocation_bytes(descriptor).expect("validated color texture descriptor");
        stats.gpu.fetch_add(bytes, Ordering::AcqRel);
        stats.textures.fetch_add(1, Ordering::AcqRel);
        cap.fetch_add(bytes, Ordering::AcqRel);
        Self { stats, cap, bytes }
    }
}

impl Drop for GpuCharge {
    fn drop(&mut self) {
        self.stats.gpu.fetch_sub(self.bytes, Ordering::AcqRel);
        self.stats.textures.fetch_sub(1, Ordering::AcqRel);
        self.cap.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct Work(Arc<Accounting>);

impl Drop for Work {
    fn drop(&mut self) {
        self.0.jobs.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct TextureJob {
    id: TextureId,
    input: PixelSource,
    texture: WeakTexture,
    revision: u64,
    cancelled: Arc<AtomicBool>,
    life: Arc<()>,
    staging: Lease,
    work: Work,
    compress: bool,
}

pub struct TextureCompletion {
    id: TextureId,
    cancelled: Arc<AtomicBool>,
    life: Arc<()>,
    data: Conversion,
    revision: u64,
}

enum Conversion {
    Ready(UploadPixels),
    Failed { failure: Failure, _work: Work },
}

struct UploadPixels {
    size: [u32; 2],
    data: UploadData,
    _staging: Lease,
    _work: Work,
}

impl UploadPixels {
    fn format(&self) -> TextureFormat {
        match &self.data {
            UploadData::Rgba(_) => TextureFormat::Rgba8UnormSrgb,
            UploadData::Bcn(image) => image.format.gpu(),
        }
    }

    fn level(&self, mip: u32) -> Option<UploadLevel<'_>> {
        match &self.data {
            UploadData::Rgba(rgba) if mip == 0 => Some(UploadLevel {
                bytes: rgba.bytes(),
                row_bytes: self.size[0] as usize * 4,
                rows: self.size[1],
                block: 1,
                width: self.size[0],
            }),
            UploadData::Rgba(_) => None,
            UploadData::Bcn(image) => {
                let level = image.levels.get(mip as usize)?;
                Some(UploadLevel {
                    bytes: &level.bytes,
                    row_bytes: level.size[0].div_ceil(4) as usize * image.format.block_bytes(),
                    rows: level.size[1].div_ceil(4),
                    block: 4,
                    width: level.size[0].next_multiple_of(4),
                })
            }
        }
    }
}

enum UploadData {
    Rgba(RgbaPixels),
    Bcn(compression::Image),
}

struct UploadLevel<'a> {
    bytes: &'a [u8],
    row_bytes: usize,
    rows: u32,
    block: u32,
    width: u32,
}

enum PixelSource {
    Image(Image),
    Decode(TextureDecodeJob),
}

enum RgbaPixels {
    Image(Image),
    Converted(Vec<u8>),
}

impl RgbaPixels {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Image(image) => image.pixels(),
            Self::Converted(bytes) => bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TextureKey {
    pub(crate) id: TextureId,
    pub(crate) sampler: Sampler,
}

struct GpuTexture {
    texture: Texture,
    view: TextureView,
    bindings: HashMap<Sampler, BindGroup>,
    charge: GpuCharge,
}

struct Upload {
    _buffer: Buffer,
    _lease: Lease,
}

enum TextureUploadState {
    Dormant,
    Failed(Failure),
    Preparing { cancelled: Arc<AtomicBool>, life: Weak<()> },
    Prepared(UploadPixels),
    Uploading { gpu: GpuTexture, pixels: UploadPixels, row: u32, mip: u32 },
    Ready(GpuTexture),
}

struct TextureEntry {
    used: u64,
    owner: WeakTexture,
    revision: u64,
    evicted: bool,
    geometry: bool,
    state: TextureUploadState,
    fallback: Option<GpuTexture>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextureState {
    pub(crate) revision: u64,
    pub(crate) staging: usize,
    pub(crate) jobs: usize,
    pub(crate) lifecycle: u64,
    pub(crate) cap: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextureViewState {
    revision: u64,
    ready: bool,
    fallback: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GeometryTextureState(pub(crate) u64);

#[derive(Clone, Copy)]
pub(crate) enum TextureUsage {
    Environment,
    Geometry,
}

pub(crate) struct Textures {
    revision: u64,
    geometry_revision: u64,
    queue: Queue,
    memory: Arc<Accounting>,
    entries: HashMap<TextureId, TextureEntry>,
    cap_bytes: Arc<AtomicUsize>,
    soft_cap: usize,
    bcn: bool,
    events: Vec<TextureEvents>,
    lifecycle: u64,
    completed: Vec<(WeakTexture, u64)>,
    retiring: Arc<AtomicUsize>,
    jobs: VecDeque<TextureJob>,
    retired: Vec<GpuTexture>,
    staged: Vec<Upload>,
    pixels: Vec<UploadPixels>,
    pub(crate) layout: BindGroupLayout,
    pub(crate) pair_layout: BindGroupLayout,
    pub(crate) default_sampler: wgpu::Sampler,
    pub(crate) water_sampler: wgpu::Sampler,
    fallback: BindGroup,
    fallback_view: TextureView,
    mip_layout: BindGroupLayout,
    mip_pipeline: RenderPipeline,
    pub(crate) uploaded: usize,
}

impl Textures {
    pub(crate) fn uploading(&self) -> bool {
        self.entries.values().any(|entry| matches!(entry.state, TextureUploadState::Uploading { .. }))
    }
    pub(crate) fn new(device: &Device, queue: &Queue) -> Self {
        static MEMORY: OnceLock<Arc<Accounting>> = OnceLock::new();
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Scene texture"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pair_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Texture pair"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let default_sampler = make_sampler(device, Sampler::default());
        let water_sampler = device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            lod_max_clamp: 32.0,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("Neutral image"),
            size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &[255; 4],
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let fallback_view = texture.create_view(&TextureViewDescriptor::default());
        let fallback = binding_with_sampler(device, &layout, &fallback_view, &default_sampler);
        let mip_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Mipmap source"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: false },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Raw channel mipmaps"),
            source: ShaderSource::Wgsl(include_str!("../shaders/mipmap.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Mipmap"),
            bind_group_layouts: &[Some(&mip_layout)],
            immediate_size: 0,
        });
        let mip_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Mipmap"),
            layout: Some(&pipeline_layout),
            vertex: VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(ColorTargetState { format: TextureFormat::Rgba8Unorm, blend: None, write_mask: ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            revision: 0,
            geometry_revision: 0,
            queue: queue.clone(),
            memory: MEMORY.get_or_init(|| Arc::new(Accounting::default())).clone(),
            entries: HashMap::new(),
            cap_bytes: Arc::new(AtomicUsize::new(0)),
            soft_cap: DEFAULT_SOFT_CAP,
            bcn: device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            events: Vec::new(),
            lifecycle: 0,
            completed: Vec::new(),
            retiring: Arc::new(AtomicUsize::new(0)),
            jobs: VecDeque::new(),
            retired: Vec::new(),
            staged: Vec::new(),
            pixels: Vec::new(),
            layout,
            pair_layout,
            default_sampler,
            water_sampler,
            fallback,
            fallback_view,
            mip_layout,
            mip_pipeline,
            uploaded: 0,
        }
    }

    pub(crate) fn request(&mut self, texture: &TextureHandle, epoch: u64, priority: Priority) {
        self.request_with_usage(texture, epoch, priority, TextureUsage::Geometry);
    }

    pub(crate) fn request_environment(&mut self, texture: &TextureHandle, epoch: u64) {
        self.request_with_usage(texture, epoch, Priority::Visible, TextureUsage::Environment);
    }

    pub(crate) fn touch(&mut self, id: TextureId, epoch: u64) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.used = epoch;
        }
    }

    fn request_with_usage(&mut self, texture: &TextureHandle, epoch: u64, priority: Priority, usage: TextureUsage) {
        let Some(info) = texture.info() else { return };
        let events = texture.events();
        if !self.events.iter().any(|registered| registered.same_source(&events)) {
            self.events.push(events);
        }
        let id = texture.id();
        let entry = self.entries.entry(id).or_insert_with(|| TextureEntry {
            used: 0,
            owner: texture.downgrade(),
            revision: info.revision,
            evicted: false,
            geometry: false,
            state: TextureUploadState::Dormant,
            fallback: None,
        });
        let geometry_added = matches!(usage, TextureUsage::Geometry) && !entry.geometry;
        entry.geometry |= matches!(usage, TextureUsage::Geometry);
        if priority == Priority::Visible {
            entry.used = epoch;
        }
        if entry.revision == info.revision && !matches!(entry.state, TextureUploadState::Dormant) {
            if geometry_added {
                self.changed(id);
            }
            return;
        }
        if entry.evicted && priority == Priority::Background {
            return;
        }
        let Some(work) = self.memory.work() else { return };
        let compress = self.bcn && compression::supported(info.size);
        let bytes = if compress { compression::work_bytes(info.size) } else { info.size[0] as usize * info.size[1] as usize * 4 };
        let Some(staging) = self.memory.reserve(bytes) else { return };
        let input = if let Some(image) = texture.image()
            && !entry.evicted
        {
            PixelSource::Image(image)
        } else {
            let Some(job) = texture.decode_job() else { return };
            PixelSource::Decode(job)
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let life = Arc::new(());
        let prior = replace(&mut entry.state, TextureUploadState::Preparing { cancelled: cancelled.clone(), life: Arc::downgrade(&life) });
        match prior {
            TextureUploadState::Ready(gpu) => entry.fallback = Some(gpu),
            TextureUploadState::Preparing { cancelled, .. } => cancelled.store(true, Ordering::Release),
            TextureUploadState::Uploading { gpu, .. } => self.retired.push(gpu),
            TextureUploadState::Dormant | TextureUploadState::Failed(_) | TextureUploadState::Prepared(_) => {}
        }
        entry.revision = info.revision;
        entry.evicted = false;
        self.jobs.push_back(TextureJob {
            id,
            input,
            texture: texture.downgrade(),
            revision: info.revision,
            cancelled,
            life,
            staging,
            work,
            compress,
        });
        self.changed(id);
    }

    pub(crate) fn next_conversion_job(&mut self) -> Option<TextureJob> {
        self.jobs.pop_front()
    }

    pub(crate) fn complete(&mut self, completion: TextureCompletion) {
        let Some(entry) = self.entries.get_mut(&completion.id) else { return };
        let TextureUploadState::Preparing { cancelled, .. } = &entry.state else { return };
        if !Arc::ptr_eq(cancelled, &completion.cancelled)
            || cancelled.load(Ordering::Acquire)
            || !entry.owner.is_alive()
            || entry.revision != completion.revision
        {
            return;
        }
        match completion.data {
            Conversion::Ready(pixels) => entry.state = TextureUploadState::Prepared(pixels),
            Conversion::Failed { failure, .. } => {
                entry.state = if failure == Failure::Cancelled { TextureUploadState::Dormant } else { TextureUploadState::Failed(failure) };
                if failure != Failure::Cancelled {
                    tracing::warn!(?failure, "texture preparation failed");
                }
            }
        }
        self.changed(completion.id);
        drop(completion.life);
    }

    fn retire_entry(&mut self, id: TextureId, evicted: bool) {
        let Some(entry) = self.entries.get_mut(&id) else { return };
        if let Some(gpu) = entry.fallback.take() {
            self.retired.push(gpu);
        }
        match replace(&mut entry.state, TextureUploadState::Dormant) {
            TextureUploadState::Preparing { cancelled, .. } => cancelled.store(true, Ordering::Release),
            TextureUploadState::Uploading { gpu, .. } | TextureUploadState::Ready(gpu) => self.retired.push(gpu),
            TextureUploadState::Dormant | TextureUploadState::Failed(_) | TextureUploadState::Prepared(_) => {}
        }
        entry.evicted = evicted;
        self.changed(id);
    }

    fn event_revision(&self) -> u64 {
        self.events.iter().fold(0u64, |total, events| total.checked_add(events.revision()).expect("texture event space exhausted"))
    }

    fn releases(&mut self) {
        let lifecycle = self.event_revision();
        if lifecycle == self.lifecycle {
            return;
        }
        self.lifecycle = lifecycle;
        let dead: Vec<_> = self.entries.iter().filter(|(_, entry)| !entry.owner.is_alive()).map(|(id, _)| *id).collect();
        for id in dead {
            self.retire_entry(id, false);
            self.entries.remove(&id);
        }
        self.jobs.retain(|job| job.texture.is_alive() && !job.cancelled.load(Ordering::Acquire));
    }

    fn trim(&mut self, epoch: u64) {
        let retiring = self.retired.iter().map(|gpu| gpu.charge.bytes).sum::<usize>() + self.retiring.load(Ordering::Acquire);
        let mut projected = self.cap_bytes.load(Ordering::Acquire).saturating_sub(retiring);
        if projected <= self.soft_cap {
            return;
        }
        let mut unused: Vec<_> =
            self.entries.iter().filter(|(_, entry)| entry.used != epoch).map(|(id, entry)| (*id, entry.used)).collect();
        unused.sort_unstable_by_key(|(id, used)| (*used, *id));
        for (id, _) in unused {
            if projected <= self.soft_cap {
                break;
            }
            let entry = &self.entries[&id];
            let bytes = entry.fallback.as_ref().map_or(0, |gpu| gpu.charge.bytes)
                + match &entry.state {
                    TextureUploadState::Ready(gpu) | TextureUploadState::Uploading { gpu, .. } => gpu.charge.bytes,
                    _ => 0,
                };
            if bytes > 0 {
                self.retire_entry(id, true);
                projected = projected.saturating_sub(bytes);
            }
        }
    }

    pub(crate) fn resident(&mut self, device: &Device, key: TextureKey) -> Option<TextureKey> {
        let entry = self.entries.get_mut(&key.id)?;
        if !entry.owner.is_alive() {
            return None;
        }
        let gpu = match &mut entry.state {
            TextureUploadState::Ready(gpu) => gpu,
            _ => entry.fallback.as_mut()?,
        };
        gpu.bindings.entry(key.sampler).or_insert_with(|| binding(device, &self.layout, &gpu.view, key.sampler));
        Some(key)
    }

    pub(crate) fn binding(&self, key: Option<TextureKey>) -> &BindGroup {
        key.and_then(|key| {
            let entry = self.entries.get(&key.id)?;
            let gpu = match &entry.state {
                TextureUploadState::Ready(gpu) => gpu,
                _ => entry.fallback.as_ref()?,
            };
            gpu.bindings.get(&key.sampler)
        })
        .unwrap_or(&self.fallback)
    }

    pub(crate) fn view(&self, key: Option<TextureKey>) -> &TextureView {
        key.and_then(|key| {
            let entry = self.entries.get(&key.id)?;
            let gpu = match &entry.state {
                TextureUploadState::Ready(gpu) => gpu,
                _ => entry.fallback.as_ref()?,
            };
            Some(&gpu.view)
        })
        .unwrap_or(&self.fallback_view)
    }

    pub(crate) fn view_state(&self, key: Option<TextureKey>) -> Option<TextureViewState> {
        let key = key?;
        Some(self.entries.get(&key.id).map_or(TextureViewState::default(), |entry| TextureViewState {
            revision: entry.revision,
            ready: matches!(&entry.state, TextureUploadState::Ready(_)),
            fallback: entry.fallback.is_some(),
        }))
    }

    pub(crate) fn pair_binding(&self, device: &Device, first: Option<TextureKey>, second: Option<TextureKey>) -> BindGroup {
        self.pair_binding_with_sampler(device, first, second, &self.default_sampler)
    }

    pub(crate) fn water_pair_binding(&self, device: &Device, first: Option<TextureKey>, second: Option<TextureKey>) -> BindGroup {
        self.pair_binding_with_sampler(device, first, second, &self.water_sampler)
    }

    fn pair_binding_with_sampler(
        &self,
        device: &Device,
        first: Option<TextureKey>,
        second: Option<TextureKey>,
        sampler: &wgpu::Sampler,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("Texture pair"),
            layout: &self.pair_layout,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureView(self.view(first)) },
                BindGroupEntry { binding: 1, resource: BindingResource::TextureView(self.view(second)) },
                BindGroupEntry { binding: 2, resource: BindingResource::Sampler(sampler) },
            ],
        })
    }

    pub(crate) fn failure(&self) -> Option<Failure> {
        self.entries.values().find_map(|entry| match entry.state {
            TextureUploadState::Failed(failure) => Some(failure),
            _ => None,
        })
    }

    pub(crate) fn count(&self) -> usize {
        self.memory.textures.load(Ordering::Acquire)
    }

    pub(crate) fn bytes(&self) -> usize {
        self.memory.gpu.load(Ordering::Acquire)
    }
    fn changed(&mut self, id: TextureId) {
        self.revision = self.revision.checked_add(1).expect("texture state space exhausted");
        if self.entries.get(&id).is_some_and(|entry| entry.geometry) {
            self.geometry_revision = self.geometry_revision.checked_add(1).expect("geometry texture state space exhausted");
        }
    }

    pub(crate) fn geometry_state(&self) -> GeometryTextureState {
        GeometryTextureState(self.geometry_revision)
    }
    pub(crate) fn state(&self) -> TextureState {
        TextureState {
            revision: self.revision,
            staging: self.staging_bytes(),
            jobs: self.memory.jobs.load(Ordering::Acquire),
            cap: self.soft_cap,
            lifecycle: self.event_revision(),
        }
    }
    pub(crate) fn soft_cap_mib(&self) -> u32 {
        (self.soft_cap / (1024 * 1024)) as u32
    }
    pub(crate) fn set_soft_cap(&mut self, mib: u32) {
        self.soft_cap = (mib as usize).saturating_mul(1024 * 1024);
    }
    pub(crate) fn staging_bytes(&self) -> usize {
        self.memory.staging.load(Ordering::Acquire)
    }

    pub(crate) fn release_resources(&mut self) {
        self.releases();
        self.defer_retired(&self.queue.clone());
    }

    fn defer_retired(&mut self, queue: &Queue) {
        if self.retired.is_empty() {
            return;
        }
        let retired = take(&mut self.retired);
        let bytes = retired.iter().map(|gpu| gpu.charge.bytes).sum::<usize>();
        self.retiring.fetch_add(bytes, Ordering::AcqRel);
        let retiring = self.retiring.clone();
        queue.on_submitted_work_done(move || {
            drop(retired);
            retiring.fetch_sub(bytes, Ordering::AcqRel);
        });
    }

    pub(crate) fn submitted(&mut self, queue: &Queue) {
        for (texture, revision) in self.completed.drain(..) {
            if let Some(texture) = texture.upgrade() {
                texture.uploaded(revision);
            }
        }
        self.defer_retired(queue);
        let staged = take(&mut self.staged);
        let pixels = take(&mut self.pixels);
        queue.on_submitted_work_done(move || {
            drop(staged);
            drop(pixels);
        });
    }
}

impl Drop for Textures {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            if let TextureUploadState::Preparing { cancelled, .. } = &entry.state {
                cancelled.store(true, Ordering::Release);
            }
        }
        let mut retired = take(&mut self.retired);
        for (_, entry) in self.entries.drain() {
            if let Some(gpu) = entry.fallback {
                retired.push(gpu);
            }
            match entry.state {
                TextureUploadState::Dormant
                | TextureUploadState::Failed(_)
                | TextureUploadState::Preparing { .. }
                | TextureUploadState::Prepared(_) => {}
                TextureUploadState::Ready(gpu) | TextureUploadState::Uploading { gpu, .. } => retired.push(gpu),
            }
        }
        let staged = take(&mut self.staged);
        let pixels = take(&mut self.pixels);
        self.queue.on_submitted_work_done(move || {
            drop(retired);
            drop(staged);
            drop(pixels);
        });
    }
}

fn mip_count(size: [u32; 2]) -> u32 {
    size[0].max(size[1]).ilog2() + 1
}

fn allocation_bytes(descriptor: &TextureDescriptor<'_>) -> Option<usize> {
    let (block_width, block_height) = descriptor.format.block_dimensions();
    let block_bytes = descriptor.format.block_copy_size(None)? as usize;
    let mut total = 0usize;
    for level in 0..descriptor.mip_level_count {
        let width = descriptor.size.width.checked_shr(level).unwrap_or(0).max(1).div_ceil(block_width) as usize;
        let height = descriptor.size.height.checked_shr(level).unwrap_or(0).max(1).div_ceil(block_height) as usize;
        let depth = if descriptor.dimension == TextureDimension::D3 {
            descriptor.size.depth_or_array_layers.checked_shr(level).unwrap_or(0).max(1)
        } else {
            descriptor.size.depth_or_array_layers
        } as usize;
        let bytes =
            width.checked_mul(height)?.checked_mul(depth)?.checked_mul(block_bytes)?.checked_mul(descriptor.sample_count as usize)?;
        total = total.checked_add(bytes)?;
    }
    Some(total)
}

fn binding(device: &Device, layout: &BindGroupLayout, view: &TextureView, settings: Sampler) -> BindGroup {
    let sampler = make_sampler(device, settings);
    binding_with_sampler(device, layout, view, &sampler)
}

fn binding_with_sampler(device: &Device, layout: &BindGroupLayout, view: &TextureView, sampler: &wgpu::Sampler) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("Scene image"),
        layout,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(view) },
            BindGroupEntry { binding: 1, resource: BindingResource::Sampler(sampler) },
        ],
    })
}

fn make_sampler(device: &Device, settings: Sampler) -> wgpu::Sampler {
    let address = |wrap| match wrap {
        Wrap::ClampToEdge => AddressMode::ClampToEdge,
        Wrap::MirroredRepeat => AddressMode::MirrorRepeat,
        Wrap::Repeat => AddressMode::Repeat,
    };
    let (min_filter, mipmap_filter, mipmaps) = match settings.min_filter {
        Some(MinFilter::Nearest) => (FilterMode::Nearest, FilterMode::Nearest, false),
        Some(MinFilter::Linear) => (FilterMode::Linear, FilterMode::Nearest, false),
        Some(MinFilter::NearestMipmapNearest) => (FilterMode::Nearest, FilterMode::Nearest, true),
        Some(MinFilter::LinearMipmapNearest) => (FilterMode::Linear, FilterMode::Nearest, true),
        Some(MinFilter::NearestMipmapLinear) => (FilterMode::Nearest, FilterMode::Linear, true),
        Some(MinFilter::LinearMipmapLinear) | None => (FilterMode::Linear, FilterMode::Linear, true),
    };
    device.create_sampler(&SamplerDescriptor {
        address_mode_u: address(settings.wrap_s),
        address_mode_v: address(settings.wrap_t),
        mag_filter: if settings.mag_filter == Some(Filter::Nearest) { FilterMode::Nearest } else { FilterMode::Linear },
        min_filter,
        mipmap_filter: if mipmap_filter == FilterMode::Nearest { MipmapFilterMode::Nearest } else { MipmapFilterMode::Linear },
        lod_max_clamp: if mipmaps { 32.0 } else { 0.0 },
        ..Default::default()
    })
}

#[cfg(test)]
mod fixtures {
    use super::*;
    use assets::{Budget, Content, Kind, LoadJob, Request, Resource, Status};
    use std::time::Instant;
    use test_support::texture::{GRAY_ALPHA_J2C, RGBA_J2C};
    use utils::AssetId;
    use uuid::Uuid;

    pub(super) fn loaded(id: u128, rgba: bool) -> TextureHandle {
        let bytes: &[u8] = if rgba { RGBA_J2C } else { GRAY_ALPHA_J2C };
        loaded_fixture(id, bytes)
    }

    pub(super) fn loaded_fixture(id: u128, bytes: &[u8]) -> TextureHandle {
        let mut store = assets::Store::new(Budget::default());
        let route = store.register_route().unwrap();
        let now = Instant::now();
        let resource = Resource { id: AssetId::new(Uuid::from_u128(id)), kind: Kind::Texture };
        let demand = store.request(route, Request::new(resource), Priority::Visible, now).unwrap();
        let LoadJob::Fetch(job) = store.next_job(now).unwrap() else { panic!("fresh texture request unexpectedly used cached source") };
        let input = job.complete(Ok(bytes.to_vec())).expect("fixture response fits");
        let completion = store.start_decode(input).expect("decode admission").run();
        store.complete(completion, now);
        let Status::Ready(Content::Texture(texture)) = demand.status() else { panic!("fixture must decode") };
        texture
    }

    pub(super) fn descriptor(size: [u32; 2], format: TextureFormat) -> TextureDescriptor<'static> {
        TextureDescriptor {
            label: None,
            size: Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: mip_count(size),
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::fixtures::descriptor;
    use super::*;

    #[test]
    fn descriptor_storage_includes_mips() {
        for (size, levels, bytes) in [([1, 1], 1, 4), ([1, 8], 4, 60), ([7, 3], 3, 100), ([2048, 2048], 12, 22_369_620)] {
            assert_eq!(mip_count(size), levels);
            assert_eq!(allocation_bytes(&descriptor(size, TextureFormat::Rgba8UnormSrgb)), Some(bytes));
        }
        for (format, bytes) in
            [(TextureFormat::Bc1RgbaUnormSrgb, 56), (TextureFormat::Bc3RgbaUnormSrgb, 112), (TextureFormat::Bc7RgbaUnormSrgb, 112)]
        {
            assert_eq!(allocation_bytes(&descriptor([8, 8], format)), Some(bytes), "block-compressed mip tails still occupy whole blocks");
        }
        let mut array = descriptor([8, 8], TextureFormat::Rgba8Unorm);
        array.size.depth_or_array_layers = 3;
        assert_eq!(allocation_bytes(&array), Some(1020));
        array.sample_count = 4;
        assert_eq!(allocation_bytes(&array), Some(4080));
    }

    #[test]
    fn staging_release_restores_admission() {
        let memory = Arc::new(Accounting::default());
        let held = memory.reserve(STAGING_BYTES).unwrap();
        assert!(memory.reserve(1).is_none());
        assert!(memory.reserve(usize::MAX).is_none());
        drop(held);
        assert!(memory.reserve(STAGING_BYTES).is_some());
    }
}
