use std::{
    error, fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use egui::{ClippedPrimitive, TexturesDelta};
use egui_wgpu::{RendererOptions, ScreenDescriptor};
use math::DVec3;
use wgpu::{
    AdapterInfo, Backends, Color, CommandEncoderDescriptor, CreateSurfaceError, CurrentSurfaceTexture, Device, DeviceDescriptor,
    ErrorFilter, InstanceDescriptor, LoadOp, Operations, PollError, PollType, PowerPreference, PresentMode, Queue,
    RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor, RenderPassTimestampWrites, RequestAdapterError,
    RequestAdapterOptions, RequestDeviceError, StoreOp, Surface, SurfaceConfiguration, SurfaceTarget, TextureViewDescriptor,
};

use crate::geometry::{GeometryRenderer, PickError, Stats};
use crate::gpu_timing::GpuTimings;
use crate::scene::{Camera, SceneView};
use crate::texture::{TextureCompletion, TextureJob};

#[derive(Debug)]
pub enum Error {
    Surface(CreateSurfaceError),
    Adapter(RequestAdapterError),
    Device(RequestDeviceError),
    Poll(PollError),
    UnsupportedSurface,
    UnsupportedColorFormat,
    UnsupportedSurfaceSize,
    Gpu(wgpu::Error),
    DeviceLost,
    SurfaceLost,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Surface(error) => write!(f, "Could not create the Vulkan surface: {error}"),
            Self::Adapter(error) => write!(f, "Could not select a Vulkan GPU: {error}"),
            Self::Device(error) => write!(f, "Could not initialize the Vulkan device: {error}"),
            Self::Poll(error) => write!(f, "Could not poll the scene GPU: {error}"),
            Self::UnsupportedSurface => f.write_str("The GPU cannot present to this window."),
            Self::UnsupportedColorFormat => f.write_str("The GPU has no supported sRGB surface format."),
            Self::UnsupportedSurfaceSize => f.write_str("The scene window exceeds the GPU's supported surface size."),
            Self::Gpu(error) => write!(f, "GPU operation failed: {error}"),
            Self::DeviceLost => f.write_str("The scene GPU device is unavailable."),
            Self::SurfaceLost => f.write_str("The scene presentation surface is unavailable."),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Surface(error) => Some(error),
            Self::Adapter(error) => Some(error),
            Self::Device(error) => Some(error),
            Self::Poll(error) => Some(error),
            Self::Gpu(error) => Some(error),
            Self::UnsupportedSurface
            | Self::UnsupportedColorFormat
            | Self::UnsupportedSurfaceSize
            | Self::DeviceLost
            | Self::SurfaceLost => None,
        }
    }
}

pub struct Renderer {
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    size: [u32; 2],
    info: AdapterInfo,
    lost: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<wgpu::Error>>>,
    overlay: egui_wgpu::Renderer,
    geometry: GeometryRenderer,
    gpu_timings: Option<GpuTimings>,
}

impl Renderer {
    pub fn retire(self) -> Retirement {
        let device = self.device.clone();
        drop(self);
        Retirement { device }
    }

    pub fn poll(&mut self) -> Result<Progress, Error> {
        self.geometry.release_textures();
        let status = self.device.poll(PollType::Poll).map_err(Error::Poll)?;
        if let Some(gpu_timings) = &mut self.gpu_timings {
            gpu_timings.poll();
        }
        if let Some(error) = self.failure.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(Error::Gpu(error));
        }
        if self.lost.load(Ordering::Acquire) {
            return Err(Error::DeviceLost);
        }
        let now = Instant::now();
        let redraw_at = self.geometry.animation_deadline().filter(|deadline| *deadline > now);
        Ok(Progress { redraw: self.geometry.needs_frame(now), pending: !status.is_queue_empty(), redraw_at })
    }
    pub async fn new(target: impl Into<SurfaceTarget<'static>>, width: u32, height: u32) -> Result<Self, Error> {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let surface = instance.create_surface(target).map_err(Error::Surface)?;
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            })
            .await
            .map_err(Error::Adapter)?;
        let info = adapter.get_info();
        let supported_features = adapter.features();
        let timestamp_feature_mask = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        let timestamp_features =
            if supported_features.contains(timestamp_feature_mask) { timestamp_feature_mask } else { wgpu::Features::empty() };
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("Radia scene"),
                required_features: (supported_features
                    & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::INDIRECT_FIRST_INSTANCE))
                    | timestamp_features,
                ..Default::default()
            })
            .await
            .map_err(Error::Device)?;
        let lost = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&lost);
        device.set_device_lost_callback(move |_, _| {
            flag.store(true, Ordering::Release);
        });
        let failure = Arc::new(Mutex::new(None));
        let reported_failure = Arc::clone(&failure);
        device.on_uncaptured_error(Arc::new(move |error| {
            let mut reported = reported_failure.lock().unwrap_or_else(PoisonError::into_inner);
            if reported.is_none() {
                *reported = Some(error);
            }
        }));
        let mut config = surface.get_default_config(&adapter, width.max(1), height.max(1)).ok_or(Error::UnsupportedSurface)?;
        let capabilities = surface.get_capabilities(&adapter);
        config.format = capabilities.formats.into_iter().find(|format| format.is_srgb()).ok_or(Error::UnsupportedColorFormat)?;
        config.present_mode =
            if capabilities.present_modes.contains(&PresentMode::Mailbox) { PresentMode::Mailbox } else { PresentMode::Fifo };
        config.desired_maximum_frame_latency = 1;
        tracing::info!(
            present_mode = ?config.present_mode,
            maximum_frame_latency = config.desired_maximum_frame_latency,
            timestamp_queries = !timestamp_features.is_empty(),
            "scene presentation configured"
        );
        let scope = device.push_error_scope(ErrorFilter::Validation);
        let overlay = egui_wgpu::Renderer::new(&device, config.format, RendererOptions::default());
        let geometry = GeometryRenderer::new(&device, &queue, config.format);
        let gpu_timings = if timestamp_features.is_empty() { None } else { GpuTimings::new(&device, queue.get_timestamp_period()) };
        if let Some(error) = scope.pop().await {
            return Err(Error::Gpu(error));
        }
        let mut renderer = Self { surface, device, queue, config, size: [0, 0], info, lost, failure, overlay, geometry, gpu_timings };
        renderer.resize(width, height);
        Ok(renderer)
    }

    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.info
    }

    pub fn stats(&self) -> Stats {
        self.geometry.stats
    }

    pub fn gpu_timings(&self) -> Option<(f64, f64)> {
        self.gpu_timings.as_ref()?.latest()
    }

    pub fn gpu_timing_supported(&self) -> bool {
        self.gpu_timings.is_some()
    }

    pub fn pick(&self, camera: Camera, ray: DVec3) -> Result<Option<DVec3>, PickError> {
        self.geometry.pick(camera, ray)
    }

    pub fn next_texture_job(&mut self) -> Option<TextureJob> {
        self.geometry.next_texture_job()
    }
    pub fn complete_texture(&mut self, completion: TextureCompletion) {
        self.geometry.complete_texture(completion);
    }
    pub fn texture_soft_cap_mib(&self) -> u32 {
        self.geometry.texture_soft_cap_mib()
    }
    pub fn set_texture_soft_cap(&mut self, mib: u32) {
        self.geometry.set_texture_soft_cap(mib);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = [width, height];
        if width == 0 || height == 0 || self.lost.load(Ordering::Acquire) {
            return;
        }
        if width > self.device.limits().max_texture_dimension_2d || height > self.device.limits().max_texture_dimension_2d {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn draw(
        &mut self,
        mut overlay: Overlay,
        scene: Option<(&SceneView, Camera)>,
        water_height: Option<f64>,
        before_present: impl FnOnce(),
    ) -> Result<bool, Error> {
        for (id, deltas) in overlay.textures.set.drain() {
            for delta in deltas {
                self.overlay.update_texture(&self.device, &self.queue, id, &delta);
            }
        }
        let result = self.draw_frame(&overlay, scene, water_height, before_present);
        for id in overlay.textures.free.drain() {
            self.overlay.free_texture(&id);
        }
        result
    }

    fn draw_frame(
        &mut self,
        overlay: &Overlay,
        scene: Option<(&SceneView, Camera)>,
        water_height: Option<f64>,
        before_present: impl FnOnce(),
    ) -> Result<bool, Error> {
        self.poll()?;
        if self.size.contains(&0) {
            return Ok(false);
        }
        if self.size.into_iter().any(|n| n > self.device.limits().max_texture_dimension_2d) {
            return Err(Error::UnsupportedSurfaceSize);
        }
        let Some(admission) = self.geometry.admit() else {
            return Ok(false);
        };
        let (frame, reconfigure) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => (frame, false),
            CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(false),
            CurrentSurfaceTexture::Outdated => {
                self.resize(self.size[0], self.size[1]);
                return Ok(false);
            }
            CurrentSurfaceTexture::Lost | CurrentSurfaceTexture::Validation => {
                return Err(Error::SurfaceLost);
            }
        };
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor { label: Some("Scene frame") });
        let timestamp_queries = self.gpu_timings.as_ref().filter(|timings| timings.recording()).map(GpuTimings::query_set);
        let screen = ScreenDescriptor { size_in_pixels: self.size, pixels_per_point: overlay.pixels_per_point };
        let commands = self.overlay.update_buffers(&self.device, &self.queue, &mut encoder, &overlay.primitives, &screen);
        let depth = if let Some((scene, camera)) = scene {
            let depth = self.geometry.depth(&self.device, self.size);
            if depth.is_some() {
                self.geometry.prepare(&self.device, &mut encoder, scene, camera, self.size, water_height);
            }
            depth
        } else {
            None
        };
        if depth.is_none() {
            self.geometry.skip();
        }
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Scene geometry"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Clear(Color { r: 0.012, g: 0.012, b: 0.012, a: 1.0 }), store: StoreOp::Store },
                })],
                depth_stencil_attachment: depth.as_ref().map(|view| RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: timestamp_queries.map(|query_set| RenderPassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
                ..Default::default()
            });
            if depth.is_some() {
                self.geometry.draw(&mut pass);
            }
        }
        {
            let pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Diagnostics"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
                })],
                timestamp_writes: timestamp_queries.map(|query_set| RenderPassTimestampWrites {
                    query_set,
                    beginning_of_pass_write_index: Some(2),
                    end_of_pass_write_index: Some(3),
                }),
                ..Default::default()
            });
            self.overlay.render(&mut pass.forget_lifetime(), &overlay.primitives, &screen);
        }
        if timestamp_queries.is_some()
            && let Some(gpu_timings) = &self.gpu_timings
        {
            gpu_timings.resolve(&mut encoder);
        }
        self.queue.submit(commands.into_iter().chain([encoder.finish()]));
        if timestamp_queries.is_some()
            && let Some(gpu_timings) = &mut self.gpu_timings
        {
            gpu_timings.submitted();
        }
        self.geometry.submitted(admission);
        before_present();
        self.queue.present(frame);
        if reconfigure {
            self.resize(self.size[0], self.size[1]);
        }
        Ok(true)
    }
}

pub struct Overlay {
    pub primitives: Vec<ClippedPrimitive>,
    pub textures: TexturesDelta,
    pub pixels_per_point: f32,
}

pub struct Progress {
    pub redraw: bool,
    pub pending: bool,
    pub redraw_at: Option<Instant>,
}

pub struct Retirement {
    device: Device,
}

impl Retirement {
    pub fn poll(&self) -> Result<bool, Error> {
        self.device.poll(PollType::Poll).map(|status| status.is_queue_empty()).map_err(Error::Poll)
    }

    pub fn wait(self) -> Result<(), Error> {
        self.device
            .poll(PollType::Wait { submission_index: None, timeout: Some(Duration::from_millis(500)) })
            .map(|_| ())
            .map_err(Error::Poll)
    }
}

#[cfg(test)]
mod tests {
    use wgpu::naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    #[test]
    fn shaders_validate() {
        for (name, source) in [
            ("surface", include_str!("shaders/surface.wgsl")),
            ("terrain", concat!(include_str!("shaders/surface.wgsl"), "\n", include_str!("shaders/terrain.wgsl"))),
            ("water", include_str!("shaders/water.wgsl")),
            ("mipmap", include_str!("shaders/mipmap.wgsl")),
        ] {
            let module = wgsl::parse_str(source).unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
            Validator::new(ValidationFlags::all(), Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }
}
