use std::{
    error, fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use egui::{ClippedPrimitive, TexturesDelta};
use egui_wgpu::{RendererOptions, ScreenDescriptor};
use math::DVec3;
use wgpu::{
    AdapterInfo, Backends, Color, CommandEncoderDescriptor, CreateSurfaceError, CurrentSurfaceTexture, Device, DeviceDescriptor,
    DisplayAndWindowHandle, ErrorFilter, Instance, InstanceDescriptor, LoadOp, Operations, PollError, PollType, PowerPreference,
    PresentMode, Queue, RenderPassColorAttachment, RenderPassDepthStencilAttachment, RenderPassDescriptor, RequestAdapterError,
    RequestAdapterOptions, RequestDeviceError, StoreOp, Surface, SurfaceConfiguration, TextureViewDescriptor,
};

use crate::geometry::{GeometryRenderer, PickError, Stats};
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
    instance: Instance,
    surface_target: Arc<dyn DisplayAndWindowHandle + Send + Sync>,
    surface: Surface<'static>,
    device: Device,
    queue: Queue,
    config: SurfaceConfiguration,
    surface_state: SurfaceState,
    info: AdapterInfo,
    lost: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<wgpu::Error>>>,
    overlay: egui_wgpu::Renderer,
    geometry: GeometryRenderer,
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
        if let Some(error) = self.failure.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Err(Error::Gpu(error));
        }
        if self.lost.load(Ordering::Acquire) {
            return Err(Error::DeviceLost);
        }
        Ok(Progress { redraw: self.geometry.needs_frame(), pending: !status.is_queue_empty() })
    }
    pub async fn new(surface_target: Arc<dyn DisplayAndWindowHandle + Send + Sync>, width: u32, height: u32) -> Result<Self, Error> {
        let instance =
            wgpu::Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let surface = instance.create_surface(Arc::clone(&surface_target)).map_err(Error::Surface)?;
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
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("Radia scene"),
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
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
        config.format =
            surface.get_capabilities(&adapter).formats.into_iter().find(|f| f.is_srgb()).ok_or(Error::UnsupportedColorFormat)?;
        config.present_mode = PresentMode::Fifo;
        config.desired_maximum_frame_latency = 2;
        let scope = device.push_error_scope(ErrorFilter::Validation);
        let overlay = egui_wgpu::Renderer::new(&device, config.format, RendererOptions::default());
        let geometry = GeometryRenderer::new(&device, &queue, config.format);
        if let Some(error) = scope.pop().await {
            return Err(Error::Gpu(error));
        }
        let mut renderer = Self {
            instance,
            surface_target,
            surface,
            device,
            queue,
            config,
            surface_state: SurfaceState::default(),
            info,
            lost,
            failure,
            overlay,
            geometry,
        };
        renderer.resize(width, height);
        Ok(renderer)
    }

    pub fn adapter_info(&self) -> &AdapterInfo {
        &self.info
    }

    pub fn stats(&self) -> Stats {
        self.geometry.stats
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
        if self.surface_state.resize(width, height) == SurfaceResize::Configure {
            self.configure_surface();
        }
    }

    pub fn draw(
        &mut self,
        mut overlay: Overlay,
        scene: Option<(&SceneView, Camera)>,
        water_height: Option<f64>,
        before_present: impl FnOnce(),
    ) -> Result<FrameOutcome, Error> {
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
    ) -> Result<FrameOutcome, Error> {
        self.poll()?;
        let Some(size) = self.surface_state.size() else {
            return Ok(FrameOutcome::Retry);
        };
        if size.into_iter().any(|n| n > self.device.limits().max_texture_dimension_2d) {
            return Err(Error::UnsupportedSurfaceSize);
        }
        let Some(admission) = self.geometry.admit() else {
            return Ok(FrameOutcome::Retry);
        };
        let (frame, reconfigure) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => (frame, false),
            CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            CurrentSurfaceTexture::Timeout => return self.surface_failure(SurfaceFailure::Timeout),
            CurrentSurfaceTexture::Occluded => return self.surface_failure(SurfaceFailure::Occluded),
            CurrentSurfaceTexture::Outdated => return self.surface_failure(SurfaceFailure::Outdated),
            CurrentSurfaceTexture::Lost => return self.surface_failure(SurfaceFailure::Lost),
            CurrentSurfaceTexture::Validation => return self.surface_failure(SurfaceFailure::Validation),
        };
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&CommandEncoderDescriptor { label: Some("Scene frame") });
        let screen = ScreenDescriptor { size_in_pixels: size, pixels_per_point: overlay.pixels_per_point };
        let commands = self.overlay.update_buffers(&self.device, &self.queue, &mut encoder, &overlay.primitives, &screen);
        let depth = if let Some((scene, camera)) = scene {
            let depth = self.geometry.depth(&self.device, size);
            if depth.is_some() {
                self.geometry.prepare(&self.device, &mut encoder, scene, camera, size, water_height);
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
                ..Default::default()
            });
            self.overlay.render(&mut pass.forget_lifetime(), &overlay.primitives, &screen);
        }
        self.queue.submit(commands.into_iter().chain([encoder.finish()]));
        self.geometry.submitted(admission);
        before_present();
        self.queue.present(frame);
        if reconfigure {
            self.surface_state.invalidate_configuration();
            self.configure_surface();
        }
        Ok(FrameOutcome::Presented)
    }

    fn surface_failure(&mut self, failure: SurfaceFailure) -> Result<FrameOutcome, Error> {
        let mut surface_state = std::mem::take(&mut self.surface_state);
        let result = surface_state.recover(failure, self);
        self.surface_state = surface_state;
        result
    }

    fn configure_surface(&mut self) {
        let mut surface_state = std::mem::take(&mut self.surface_state);
        surface_state.configure(self);
        self.surface_state = surface_state;
    }

    fn configure_surface_size(&mut self, [width, height]: [u32; 2]) -> bool {
        if self.lost.load(Ordering::Acquire)
            || width > self.device.limits().max_texture_dimension_2d
            || height > self.device.limits().max_texture_dimension_2d
        {
            return false;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        true
    }
}

impl SurfaceRecoveryTarget for Renderer {
    fn configure(&mut self, size: [u32; 2]) -> bool {
        self.configure_surface_size(size)
    }

    fn recreate(&mut self, size: [u32; 2]) -> Result<bool, Error> {
        self.surface = self.instance.create_surface(Arc::clone(&self.surface_target)).map_err(Error::Surface)?;
        Ok(self.configure_surface_size(size))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    Retry,
    Occluded,
}

#[derive(Default)]
struct SurfaceState {
    size: Option<[u32; 2]>,
    configured_size: Option<[u32; 2]>,
}

impl SurfaceState {
    fn resize(&mut self, width: u32, height: u32) -> SurfaceResize {
        if width == 0 || height == 0 {
            self.size = None;
            self.configured_size = None;
            SurfaceResize::Minimized
        } else {
            self.size = Some([width, height]);
            self.configured_size = None;
            SurfaceResize::Configure
        }
    }

    fn size(&self) -> Option<[u32; 2]> {
        self.size
    }

    fn pending_configuration(&self) -> Option<[u32; 2]> {
        self.size.filter(|size| self.configured_size != Some(*size))
    }

    fn mark_configured(&mut self, size: [u32; 2]) {
        self.configured_size = Some(size);
    }

    fn invalidate_configuration(&mut self) {
        self.configured_size = None;
    }

    fn configure(&mut self, target: &mut impl SurfaceRecoveryTarget) {
        if let Some(size) = self.pending_configuration()
            && target.configure(size)
        {
            self.mark_configured(size);
        }
    }

    fn recover(&mut self, failure: SurfaceFailure, target: &mut impl SurfaceRecoveryTarget) -> Result<FrameOutcome, Error> {
        match failure {
            SurfaceFailure::Timeout => Ok(FrameOutcome::Retry),
            SurfaceFailure::Occluded => Ok(FrameOutcome::Occluded),
            SurfaceFailure::Outdated => {
                self.invalidate_configuration();
                self.configure(target);
                Ok(FrameOutcome::Retry)
            }
            SurfaceFailure::Lost => {
                self.invalidate_configuration();
                if let Some(size) = self.size
                    && target.recreate(size)?
                {
                    self.mark_configured(size);
                }
                Ok(FrameOutcome::Retry)
            }
            SurfaceFailure::Validation => Err(Error::SurfaceLost),
        }
    }
}

trait SurfaceRecoveryTarget {
    fn configure(&mut self, size: [u32; 2]) -> bool;
    fn recreate(&mut self, size: [u32; 2]) -> Result<bool, Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceResize {
    Minimized,
    Configure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceFailure {
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

pub struct Overlay {
    pub primitives: Vec<ClippedPrimitive>,
    pub textures: TexturesDelta,
    pub pixels_per_point: f32,
}

pub struct Progress {
    pub redraw: bool,
    pub pending: bool,
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
    use super::{Error, FrameOutcome, SurfaceFailure, SurfaceRecoveryTarget, SurfaceResize, SurfaceState};
    use wgpu::naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    #[derive(Default)]
    struct TestSurface {
        configured_size: Option<[u32; 2]>,
        recreations: usize,
    }

    impl SurfaceRecoveryTarget for TestSurface {
        fn configure(&mut self, size: [u32; 2]) -> bool {
            self.configured_size = Some(size);
            true
        }

        fn recreate(&mut self, size: [u32; 2]) -> Result<bool, Error> {
            self.recreations += 1;
            self.configured_size = None;
            Ok(self.configure(size))
        }
    }

    #[test]
    fn restore_requests_configuration() {
        let mut state = SurfaceState::default();
        let mut surface = TestSurface::default();
        assert_eq!(state.resize(0, 0), SurfaceResize::Minimized);
        assert_eq!(state.size(), None);
        assert_eq!(state.resize(1280, 720), SurfaceResize::Configure);
        state.configure(&mut surface);
        assert_eq!(surface.configured_size, Some([1280, 720]));
        assert_eq!(state.pending_configuration(), None);
    }

    #[test]
    fn outdated_surface_is_reconfigured() {
        let (mut state, mut surface) = configured_surface();
        std::assert_matches!(state.recover(SurfaceFailure::Outdated, &mut surface), Ok(FrameOutcome::Retry));
        assert_eq!(surface.configured_size, Some([1280, 720]));
        assert_eq!(surface.recreations, 0);
        assert_eq!(state.pending_configuration(), None);
    }

    #[test]
    fn lost_surface_is_recreated_and_reconfigured() {
        let (mut state, mut surface) = configured_surface();
        std::assert_matches!(state.recover(SurfaceFailure::Lost, &mut surface), Ok(FrameOutcome::Retry));
        assert_eq!(surface.recreations, 1);
        assert_eq!(surface.configured_size, Some([1280, 720]));
        assert_eq!(state.pending_configuration(), None);
    }

    #[test]
    fn occlusion_waits_for_visibility_and_timeout_retries() {
        let (mut state, mut surface) = configured_surface();
        std::assert_matches!(state.recover(SurfaceFailure::Occluded, &mut surface), Ok(FrameOutcome::Occluded));
        std::assert_matches!(state.recover(SurfaceFailure::Timeout, &mut surface), Ok(FrameOutcome::Retry));
        assert_eq!(surface.configured_size, Some([1280, 720]));
        assert_eq!(surface.recreations, 0);
        assert_eq!(state.pending_configuration(), None);
    }

    #[test]
    fn validation_failure_requests_renderer_recovery() {
        let (mut state, mut surface) = configured_surface();
        std::assert_matches!(state.recover(SurfaceFailure::Validation, &mut surface), Err(Error::SurfaceLost));
    }

    fn configured_surface() -> (SurfaceState, TestSurface) {
        let mut state = SurfaceState::default();
        state.resize(1280, 720);
        let mut surface = TestSurface::default();
        state.configure(&mut surface);
        (state, surface)
    }

    #[test]
    fn shaders_validate() {
        for (name, source) in [
            ("surface", include_str!("shaders/surface.wgsl")),
            ("water", include_str!("shaders/water.wgsl")),
            ("mipmap", include_str!("shaders/mipmap.wgsl")),
        ] {
            let module = wgsl::parse_str(source).unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
            Validator::new(ValidationFlags::all(), Capabilities::all()).validate(&module).unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }
}
