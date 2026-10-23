use std::{
    io,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    DEFAULT_DRAW_DISTANCE,
    camera::{self, OrbitCamera},
    scene_feed::SceneFeed,
};
use egui::{Context, DragValue, Slider, ViewportId};
use futures::{
    channel::mpsc::{Receiver, Sender, channel},
    executor::block_on,
};
use render::{CompressionQuality, Overlay, Renderer, Retirement, TextureCompletion};
use tokio::runtime::Handle;
use tokio::sync::watch;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorGrabMode, Window, WindowId},
};

#[cfg(target_os = "linux")]
use winit::platform::wayland::EventLoopBuilderExtWayland;
#[cfg(target_os = "windows")]
use winit::platform::windows::EventLoopBuilderExtWindows;

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const RECOVERY_DELAY: Duration = Duration::from_millis(500);
const MAX_RECOVERIES: u32 = 3;

pub(crate) enum ViewportEvent {
    Ready(u64),
    Chat(u64),
    Closed(u64),
    Failed(u64, String),
}

#[derive(Clone, Copy, Default)]
struct RequestedScene {
    generation: u64,
    visible: bool,
}

impl RequestedScene {
    fn show(&mut self, generation: u64) -> bool {
        if generation > self.generation {
            *self = Self { generation, visible: true };
            true
        } else {
            false
        }
    }

    fn clear(&mut self, generation: u64) {
        if self.generation == generation {
            self.visible = false;
        }
    }
}

#[derive(Default)]
struct SharedWindowState {
    requested: Mutex<RequestedScene>,
    feed: Mutex<Option<(u64, SceneFeed)>>,
    proxy: Mutex<Option<EventLoopProxy<()>>>,
    wake_pending: AtomicBool,
    chat_pending: AtomicBool,
    stop: AtomicBool,
}

impl SharedWindowState {
    fn wake(&self) -> bool {
        let proxy = self.proxy.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(proxy) = &*proxy else {
            return true;
        };
        if self.wake_pending.swap(true, Ordering::AcqRel) {
            return true;
        }
        proxy.send_event(()).is_ok()
    }
}

pub(crate) struct Viewport {
    shared: Arc<SharedWindowState>,
    worker: Option<JoinHandle<()>>,
}

impl Viewport {
    pub(crate) fn start() -> io::Result<(Self, Receiver<ViewportEvent>)> {
        let (events, incoming) = channel(8);
        let shared = Arc::new(SharedWindowState::default());
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new().name("scene-window".into()).spawn(move || {
            let event_loop = match EventLoop::with_user_event().with_any_thread(true).build() {
                Ok(event_loop) => event_loop,
                Err(error) => {
                    tracing::error!(%error, "could not start scene event loop");
                    return;
                }
            };
            *worker_shared.proxy.lock().unwrap_or_else(PoisonError::into_inner) = Some(event_loop.create_proxy());
            let mut app = WindowApp { shared: Arc::clone(&worker_shared), events, active: None, retired: Vec::new(), generation: 0 };
            if let Err(error) = event_loop.run_app(&mut app) {
                tracing::error!(%error, "scene event loop failed");
            }
            app.retire_active();
            for retired in app.retired.drain(..) {
                if let Err(error) = retired.wait() {
                    tracing::warn!(%error, "scene GPU retirement did not finish");
                }
            }
            *worker_shared.proxy.lock().unwrap_or_else(PoisonError::into_inner) = None;
        })?;
        Ok((Self { shared, worker: Some(worker) }, incoming))
    }

    pub(crate) fn show(&self, generation: u64) -> bool {
        {
            let mut requested = self.shared.requested.lock().unwrap_or_else(PoisonError::into_inner);
            if !requested.show(generation) {
                return false;
            }
        }
        self.worker.as_ref().is_some_and(|worker| !worker.is_finished()) && self.shared.wake()
    }

    pub(crate) fn clear(&self, generation: u64) {
        {
            let mut requested = self.shared.requested.lock().unwrap_or_else(PoisonError::into_inner);
            requested.clear(generation);
            let mut feed = self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner);
            if feed.as_ref().is_some_and(|(g, _)| *g == generation) {
                *feed = None;
            }
        }
        self.shared.wake();
    }

    pub(crate) fn chat_received(&self) {
        self.shared.chat_pending.store(false, Ordering::Release);
    }

    pub(crate) fn connect_world(
        &self,
        generation: u64,
        sources: crate::scene_feed::SceneSources,
        agent_id: uuid::Uuid,
        camera: watch::Sender<Option<scene::Camera>>,
        runtime: Handle,
    ) {
        let requested = self.shared.requested.lock().unwrap_or_else(PoisonError::into_inner);
        if requested.generation == generation && requested.visible {
            let shared = Arc::clone(&self.shared);
            let feed = SceneFeed::start(sources, agent_id, camera, runtime, move || {
                shared.wake();
            });
            *self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner) = Some((generation, feed));
            self.shared.wake();
        }
    }

    pub(crate) fn shutdown(mut self) {
        self.shared.stop.store(true, Ordering::Release);
        *self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.shared.wake();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!("scene window worker panicked");
        }
    }
}

impl Drop for Viewport {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        *self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.shared.wake();
    }
}

struct WindowApp {
    shared: Arc<SharedWindowState>,
    events: Sender<ViewportEvent>,
    active: Option<SceneWindow>,
    retired: Vec<Retirement>,
    generation: u64,
}

impl WindowApp {
    fn retire_active(&mut self) {
        if let Some(mut active) = self.active.take() {
            if let Some(renderer) = active.renderer.take() {
                self.retired.push(renderer.retire());
            }
            if let Some(retired) = active.retired.take() {
                self.retired.push(retired);
            }
        }
    }

    fn poll_retired(&mut self) {
        self.retired.retain(|retired| match retired.poll() {
            Ok(done) => !done,
            Err(error) => {
                tracing::warn!(%error, "scene GPU retirement failed");
                false
            }
        });
    }
    fn publish(&mut self, event: ViewportEvent, event_loop: &ActiveEventLoop) {
        if self.events.try_send(event).is_err() {
            event_loop.exit();
        }
    }

    fn synchronize(&mut self, event_loop: &ActiveEventLoop) {
        if self.shared.stop.load(Ordering::Acquire) {
            self.retire_active();
            *self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner) = None;
            event_loop.exit();
            return;
        }
        let requested = *self.shared.requested.lock().unwrap_or_else(PoisonError::into_inner);
        if !requested.visible {
            self.retire_active();
            *self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner) = None;
            return;
        }
        if requested.generation > self.generation {
            self.retire_active();
            if self.retired.len() >= MAX_RECOVERIES as usize {
                return;
            }
            self.generation = requested.generation;
            match SceneWindow::new(event_loop, requested.generation, Arc::clone(&self.shared)) {
                Ok(window) => self.active = Some(window),
                Err(error) => self.publish(ViewportEvent::Failed(requested.generation, error), event_loop),
            }
        }
        let incoming = self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(active) = &mut self.active {
            if let Some((generation, feed)) = incoming
                && generation == active.generation
            {
                active.feed = Some(feed);
                active.dirty = true;
            }
            if let Some(feed) = &mut active.feed {
                let before = feed.view.version();
                if let Err(error) = feed.drain() {
                    let generation = active.generation;
                    self.retire_active();
                    self.publish(ViewportEvent::Failed(generation, format!("Scene publication failed: {error:?}")), event_loop);
                } else {
                    active.dirty |= before != feed.view.version();
                }
            }
        }
    }

    fn request_chat(&mut self, generation: u64) {
        if !self.shared.chat_pending.swap(true, Ordering::AcqRel) {
            let _ = self.events.try_send(ViewportEvent::Chat(generation));
        }
    }
}

impl ApplicationHandler for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.synchronize(event_loop);
        if let Some(active) = &mut self.active {
            active.dirty = true;
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.shared.wake_pending.store(false, Ordering::Release);
        self.synchronize(event_loop);
        if let Some(active) = &mut self.active {
            active.dirty = true;
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && let Some(active) = &mut self.active
            && let (Some(drag), Some(camera)) = (&mut active.drag, &mut active.camera)
        {
            drag.travel[0] += dx.abs() / active.dpi;
            drag.travel[1] += dy.abs() / active.dpi;
            if drag.travel[0] >= 4.0 || drag.travel[1] >= 4.0 {
                active.dirty = true;
                camera.drag(
                    [dx, if drag.travel[1] >= 4.0 { dy } else { 0.0 }],
                    f64::from(active.window.inner_size().width),
                    active.modifiers.control_key(),
                    active.modifiers.shift_key(),
                );
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.window.id() != id {
            return;
        }
        active.dirty = true;
        let response = active.platform.on_window_event(&active.window, &event);
        match event {
            WindowEvent::ModifiersChanged(modifiers) => active.modifiers = modifiers.state(),
            WindowEvent::Focused(false) => {
                active.release_drag(false);
                active.modifiers = ModifiersState::empty();
            }
            WindowEvent::CursorMoved { position, .. } => {
                active.cursor = position;
            }
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                if state == ElementState::Released {
                    active.release_drag(true);
                } else if active.modifiers.alt_key()
                    && !response.consumed
                    && !active.egui.egui_wants_pointer_input()
                    && let (Some(renderer), Some(observation), Some(camera)) = (&active.renderer, active.last_camera, &mut active.camera)
                {
                    let size = active.window.inner_size();
                    match camera::pick_world_point(
                        renderer.instances(),
                        observation,
                        [active.cursor.x, active.cursor.y],
                        [size.width, size.height],
                    ) {
                        Ok(Some(point)) => {
                            camera.focus(point, Instant::now());
                            let origin = active.cursor;
                            let center = PhysicalPosition::new(f64::from(size.width) * 0.5, f64::from(size.height) * 0.5);
                            match active.window.set_cursor_grab(CursorGrabMode::Confined) {
                                Ok(()) => {
                                    active.window.set_cursor_visible(false);
                                    active.drag = Some(DragState { origin, travel: [0.0; 2] });
                                    if let Err(error) = active.window.set_cursor_position(center) {
                                        active.release_drag(false);
                                        tracing::warn!(%error, "could not position scene cursor");
                                    }
                                }
                                Err(error) => tracing::warn!(%error, "could not capture scene cursor"),
                            }
                        }
                        Ok(None) => {}
                        Err(()) => tracing::debug!("scene focus pick exceeded its work limit"),
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } if !response.consumed && !active.egui.egui_wants_pointer_input() => {
                if let Some(camera) = &mut active.camera {
                    let clicks = match delta {
                        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                        MouseScrollDelta::PixelDelta(position) => position.y / (40.0 * active.dpi),
                    };
                    camera.zoom(2.0f64.powf(-clicks / 4.0));
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
                    && !response.consumed
                    && !active.egui.egui_wants_keyboard_input() =>
            {
                active.release_drag(true);
                if let Some(camera) = &mut active.camera {
                    camera.reset();
                }
            }
            WindowEvent::CloseRequested => {
                let generation = active.generation;
                self.retire_active();
                self.publish(ViewportEvent::Closed(generation), event_loop);
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut active.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::Occluded(occluded) => active.occluded = occluded,
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat && event.logical_key == Key::Named(NamedKey::F6) =>
            {
                let generation = active.generation;
                self.request_chat(generation);
            }
            WindowEvent::RedrawRequested => {
                let generation = active.generation;
                match active.draw() {
                    Ok((first_frame, chat)) => {
                        if first_frame {
                            self.publish(ViewportEvent::Ready(generation), event_loop);
                        }
                        if chat {
                            self.request_chat(generation);
                        }
                    }
                    Err(error) => {
                        self.retire_active();
                        self.publish(ViewportEvent::Failed(generation, error), event_loop);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_retired();
        self.synchronize(event_loop);
        let Some(active) = &mut self.active else {
            event_loop.set_control_flow(if self.retired.is_empty() {
                ControlFlow::Wait
            } else {
                ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL)
            });
            return;
        };
        if let Some(retired) = active.retired.take() {
            self.retired.push(retired);
        }
        let mut pending = !self.retired.is_empty();
        if let Some(renderer) = &mut active.renderer {
            match renderer.poll() {
                Ok(progress) => {
                    active.dirty |= progress.redraw;
                    pending |= progress.pending;
                }
                Err(error) => {
                    tracing::warn!(%error, "scene GPU failed; scheduling recovery");
                    active.retire_renderer();
                    active.dirty = true;
                    active.next_frame = Instant::now() + RECOVERY_DELAY;
                    pending = true;
                }
            }
        }
        if active.occluded || active.window.inner_size().width == 0 || active.window.inner_size().height == 0 {
            event_loop.set_control_flow(if pending { ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL) } else { ControlFlow::Wait });
            return;
        }
        let now = Instant::now();
        if active.next_ui.is_some_and(|at| now >= at) {
            active.next_ui = None;
            active.dirty = true;
        }
        if active.dirty && now >= active.next_frame && self.retired.len() < MAX_RECOVERIES as usize {
            active.window.request_redraw();
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            let redraw = (active.dirty && self.retired.len() < MAX_RECOVERIES as usize)
                .then_some(active.next_frame.max(now + Duration::from_millis(1)));
            let poll = pending.then_some(now + FRAME_INTERVAL);
            event_loop.set_control_flow(match redraw.into_iter().chain(poll).chain(active.next_ui).min() {
                Some(at) => ControlFlow::WaitUntil(at),
                None => ControlFlow::Wait,
            });
        }
    }
}

struct SceneWindow {
    wake: Arc<SharedWindowState>,
    dirty: bool,
    retired: Option<Retirement>,
    texture_sender: mpsc::SyncSender<TextureCompletion>,
    texture_receiver: mpsc::Receiver<TextureCompletion>,
    feed: Option<SceneFeed>,
    renderer: Option<Renderer>,
    texture_soft_cap: u32,
    egui: Context,
    platform: egui_winit::State,
    next_ui: Option<Instant>,
    window: Arc<Window>,
    generation: u64,
    frame_work: Duration,
    next_frame: Instant,
    dpi: f64,
    occluded: bool,
    ready: bool,
    recoveries: u32,
    recovery: PresentationRecovery,
    camera: Option<OrbitCamera>,
    last_camera: Option<scene::Camera>,
    cursor: PhysicalPosition<f64>,
    modifiers: ModifiersState,
    drag: Option<DragState>,
}

struct DragState {
    origin: PhysicalPosition<f64>,
    travel: [f64; 2],
}

impl SceneWindow {
    fn new(event_loop: &ActiveEventLoop, generation: u64, wake: Arc<SharedWindowState>) -> Result<Self, String> {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Radia")
                        .with_inner_size(LogicalSize::new(1280, 720))
                        .with_min_inner_size(LogicalSize::new(460, 300))
                        .with_visible(false),
                )
                .map_err(|e| e.to_string())?,
        );
        let (egui, platform) = debug_ui(&window);
        let dpi = window.scale_factor();
        let size = window.inner_size();
        let renderer = block_on(Renderer::new(Arc::clone(&window), size.width, size.height)).map_err(|e| e.to_string())?;
        let info = renderer.adapter_info();
        tracing::info!(backend = ?info.backend, gpu = %info.name, driver = %info.driver, driver_info = %info.driver_info, "scene GPU initialized");
        window.set_visible(true);
        let now = Instant::now();
        let texture_soft_cap = renderer.texture_soft_cap_mib();
        let (texture_sender, texture_receiver) = mpsc::sync_channel(2);
        Ok(Self {
            wake,
            dirty: true,
            retired: None,
            texture_sender,
            texture_receiver,
            feed: None,
            renderer: Some(renderer),
            texture_soft_cap,
            egui,
            platform,
            next_ui: None,
            window,
            generation,
            frame_work: Duration::ZERO,
            next_frame: now,
            dpi,
            occluded: false,
            ready: false,
            recoveries: 0,
            recovery: PresentationRecovery::default(),
            camera: None,
            last_camera: None,
            cursor: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            drag: None,
        })
    }

    fn release_drag(&mut self, restore: bool) {
        if let Some(drag) = self.drag.take() {
            if let Err(error) = self.window.set_cursor_grab(CursorGrabMode::None) {
                tracing::warn!(%error, "could not release scene cursor capture");
            }
            self.window.set_cursor_visible(true);
            if restore && let Err(error) = self.window.set_cursor_position(drag.origin) {
                tracing::warn!(%error, "could not restore scene cursor position");
            }
        }
    }

    fn draw(&mut self) -> Result<(bool, bool), String> {
        if self.camera.is_none()
            && let Some(feed) = &self.feed
            && let Some(object) = feed.view.objects().find(|object| object.detail().summary.id == feed.agent_id)
            && let Ok(world::WorldTransformState::Resolved(transform)) = object.detail().world_transform
        {
            self.camera = Some(OrbitCamera::new(transform.pose.position));
        }
        let observation = self.camera.as_ref().and_then(|camera| camera.scene_camera(self.window.inner_size().into(), Instant::now()));
        if let (Some(feed), Some(camera)) = (&self.feed, observation) {
            feed.set_camera(camera);
        }
        let now = Instant::now();
        if now < self.next_frame {
            return Ok((false, false));
        }
        self.dirty = false;
        if self.renderer.is_none() {
            if !self.recovery.begin_attempt() {
                return Err("The scene GPU could not recover after three attempts.".into());
            }
            let size = self.window.inner_size();
            match block_on(Renderer::new(Arc::clone(&self.window), size.width, size.height)) {
                Ok(renderer) => {
                    (self.texture_sender, self.texture_receiver) = mpsc::sync_channel(2);
                    let info = renderer.adapter_info();
                    tracing::info!(backend = ?info.backend, gpu = %info.name, driver = %info.driver, driver_info = %info.driver_info, "scene GPU recovered");
                    (self.egui, self.platform) = debug_ui(&self.window);
                    self.renderer = Some(renderer);
                    self.recoveries += 1;
                }
                Err(error) => {
                    tracing::warn!(%error, attempt = self.recovery.attempts, "scene GPU recovery failed");
                    self.next_frame = now + RECOVERY_DELAY;
                    self.dirty = true;
                    return Ok((false, false));
                }
            }
        }
        let renderer = self.renderer.as_mut().expect("renderer initialized above");
        while let Ok(completion) = self.texture_receiver.try_recv() {
            renderer.complete_texture(completion);
        }
        self.dpi = self.window.scale_factor();
        let input = self.platform.take_egui_input(&self.window);
        let mut chat = false;
        let mut recreate = false;
        let output = self.egui.run_ui(input, |root| {
            egui::Window::new("Rendering").default_width(400.0).show(root.ctx(), |ui| {
                if let Some(camera) = &mut self.camera {
                    ui.add(Slider::new(&mut camera.draw_distance, 32.0..=1024.0).text("Draw distance").suffix(" m"));
                    ui.add(Slider::new(&mut camera.lod_factor, 0.25..=4.0).text("LOD factor"))
                        .on_hover_text("Higher values keep more detailed prim geometry at a distance.");
                    if ui.button("Reset settings").clicked() {
                        camera.draw_distance = DEFAULT_DRAW_DISTANCE;
                        camera.lod_factor = 1.0;
                    }
                }
                ui.label(format!("CPU frame: {:.2} ms", self.frame_work.as_secs_f64() * 1000.0));
                let stats = renderer.stats();
                ui.label(format!("Draws: {}, instances: {}", stats.draw_calls, stats.instances));
                ui.collapsing("Diagnostics", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(DragValue::new(&mut self.texture_soft_cap).speed(16).suffix(" MiB"))
                            .on_hover_text("Soft GPU backstop. Textures used by the current frame stay resident, even above this value.");
                        ui.label("Texture soft cap");
                    });
                    ui.label(format!("CPU prepare: {:.2} ms", stats.prepare_ms));
                    ui.label(format!("GPU meshes: {:.1} MiB, pending: {}", stats.mesh_bytes as f64 / 1048576.0, stats.pending_meshes));
                    ui.label(format!(
                        "GPU textures: {}, {:.1} MiB, pending: {}",
                        stats.texture_count,
                        stats.texture_bytes as f64 / 1048576.0,
                        stats.pending_textures
                    ));
                    ui.label(format!(
                        "Texture staging: {:.1} MiB, upload: {:.1} MiB",
                        stats.staging_bytes as f64 / 1048576.0,
                        stats.texture_upload_bytes as f64 / 1048576.0
                    ));
                    if stats.omitted_faces > 0 {
                        ui.label(format!("Faces over draw budget: {}", stats.omitted_faces));
                    }
                    ui.label(format!("GPU depth: {:.1} MiB", stats.depth_bytes as f64 / 1048576.0));
                    if let Some(feed) = &self.feed {
                        ui.label(format!(
                            "Objects: {}, regions: {}, terrain patches: {}",
                            feed.view.object_count(),
                            feed.view.region_count(),
                            feed.view.patch_count()
                        ));
                        if let Some(revision) = feed.view.revision() {
                            ui.label(format!("World revision: {}", revision.get()));
                        }
                        ui.label(format!(
                            "Scene: {:.1} MiB, peak {:.1} MiB",
                            feed.budget.charged_bytes() as f64 / 1048576.0,
                            feed.budget.high_water_bytes() as f64 / 1048576.0
                        ));
                    }
                    if self.recoveries > 0 {
                        ui.label(format!("GPU recreations: {}", self.recoveries));
                    }
                    recreate = ui.button("Recreate GPU resources").clicked();
                });
                chat = ui.button("Nearby chat (F6)").clicked();
            });
        });
        self.next_ui = output.viewport_output.get(&ViewportId::ROOT).and_then(|viewport| now.checked_add(viewport.repaint_delay));
        self.platform.handle_platform_output(&self.window, output.platform_output);
        if self.drag.is_some() {
            self.window.set_cursor_visible(false);
        }
        renderer.set_texture_soft_cap(self.texture_soft_cap);
        let scene = self.feed.as_ref().zip(observation).map(|(feed, camera)| (&feed.view, camera));
        let water_height = self.feed.as_ref().and_then(|feed| feed.water_height);
        let overlay = Overlay {
            primitives: self.egui.tessellate(output.shapes, output.pixels_per_point),
            textures: output.textures_delta,
            pixels_per_point: output.pixels_per_point,
        };
        let result = renderer.draw(overlay, scene, water_height, || self.window.pre_present_notify());
        if let Some(feed) = &self.feed {
            while let Some(job) = renderer.next_texture_job() {
                let sender = self.texture_sender.clone();
                let wake = Arc::clone(&self.wake);
                let work = feed.runtime.spawn_blocking(move || job.run(CompressionQuality::Fast));
                drop(feed.runtime.spawn(async move {
                    match work.await {
                        Ok(completion) => match sender.try_send(completion) {
                            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
                            Err(mpsc::TrySendError::Full(_)) => tracing::warn!("Texture completion queue is full"),
                        },
                        Err(error) => tracing::error!(%error, "texture worker stopped unexpectedly"),
                    }
                    wake.wake();
                }));
            }
        }
        self.next_frame = Instant::now();
        self.frame_work = self.next_frame.duration_since(now);
        match result {
            Ok(presented) => {
                self.dirty |= !presented || self.camera.as_ref().is_some_and(|camera| camera.moving(Instant::now()));
                self.dirty |= self.camera.as_ref().and_then(|camera| camera.scene_camera(self.window.inner_size().into(), Instant::now()))
                    != observation;
                if presented {
                    self.last_camera = observation;
                }
                if !presented {
                    self.next_frame += FRAME_INTERVAL;
                }
                let first_frame = presented && !self.ready;
                self.ready |= presented;
                if presented {
                    self.recovery.presented();
                }
                if recreate {
                    self.retire_renderer();
                    self.dirty = true;
                    self.next_frame = now + RECOVERY_DELAY;
                }
                Ok((first_frame, chat))
            }
            Err(error) => {
                tracing::warn!(%error, "scene presentation failed; scheduling GPU recovery");
                self.retire_renderer();
                self.dirty = true;
                self.next_frame = now + RECOVERY_DELAY;
                Ok((false, chat))
            }
        }
    }

    fn retire_renderer(&mut self) {
        if let Some(renderer) = self.renderer.take() {
            self.retired = Some(renderer.retire());
        }
    }
}

impl Drop for SceneWindow {
    fn drop(&mut self) {
        self.release_drag(false);
    }
}

#[derive(Default)]
struct PresentationRecovery {
    attempts: u32,
}

impl PresentationRecovery {
    fn begin_attempt(&mut self) -> bool {
        if self.attempts == MAX_RECOVERIES {
            return false;
        }
        self.attempts += 1;
        true
    }

    fn presented(&mut self) {
        self.attempts = 0;
    }
}

fn debug_ui(window: &Window) -> (Context, egui_winit::State) {
    let ctx = Context::default();
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let state = egui_winit::State::new(ctx.clone(), ViewportId::ROOT, window, Some(window.scale_factor() as f32), window.theme(), None);
    (ctx, state)
}

#[cfg(test)]
mod tests {
    use super::{PresentationRecovery, RequestedScene};

    #[test]
    fn requests_cannot_revive_cancelled_sessions() {
        let mut requested = RequestedScene::default();
        for generation in 1..=100 {
            requested.show(generation);
        }
        requested.clear(99);
        assert_eq!(requested.generation, 100);
        assert!(requested.visible, "late cancellation must leave the current session visible");
        requested.clear(100);
        requested.show(100);
        requested.show(99);
        assert!(!requested.visible, "neither repeated nor stale startup may revive a cancelled scene");
        requested.show(101);
        assert!(requested.visible);
        assert_eq!(requested.generation, 101);
    }

    #[test]
    fn presentation_resets_recovery_limit() {
        let mut recovery = PresentationRecovery::default();
        for _ in 0..6 {
            assert!(recovery.begin_attempt());
            recovery.presented();
        }
        assert!(recovery.begin_attempt());
        assert!(recovery.begin_attempt());
        assert!(recovery.begin_attempt());
        assert!(!recovery.begin_attempt(), "three attempts without a presented frame exhaust recovery");
    }
}
