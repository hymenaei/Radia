use std::{
    error::Error,
    fmt,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use egui::{Context, DragValue, Slider, ViewportId};
use futures::executor::block_on;
use render::scene;
use render::{CompressionQuality, Overlay, Renderer, Retirement, TextureCompletion};
use tracing::Instrument;
#[cfg(target_os = "linux")]
use winit::platform::startup_notify::{EventLoopExtStartupNotify, WindowAttributesExtStartupNotify, reset_activation_token_env};
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorGrabMode, UserAttentionType, Window, WindowId},
};

use super::{
    FRAME_INTERVAL, InputCapture, SharedWindowState,
    timing::{FrameTimes, next_frame_deadline, redraw_interval, refresh_interval},
};
use crate::{
    DEFAULT_DRAW_DISTANCE,
    agent_movement::{BodyRotation, HeldControls, MovementObservation, STOP},
    camera::{CameraMode, FollowCamera, OrbitCamera, handle_keyboard_input},
    scene_feed::SceneFeed,
};

const RECOVERY_DELAY: Duration = Duration::from_millis(500);
pub(super) const MAX_RECOVERIES: u32 = 3;

fn display_refresh_interval(window: &Window) -> Duration {
    let millihertz = window.current_monitor().and_then(|monitor| monitor.refresh_rate_millihertz());
    refresh_interval(millihertz, FRAME_INTERVAL)
}

#[derive(Debug)]
pub(crate) enum SceneWindowError {
    WindowCreation(winit::error::OsError),
    Renderer(render::Error),
    RecoveryExhausted,
}

impl fmt::Display for SceneWindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowCreation(error) => fmt::Display::fmt(error, f),
            Self::Renderer(error) => fmt::Display::fmt(error, f),
            Self::RecoveryExhausted => f.write_str("The scene GPU could not recover after three attempts."),
        }
    }
}

impl Error for SceneWindowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::WindowCreation(error) => Some(error),
            Self::Renderer(error) => Some(error),
            Self::RecoveryExhausted => None,
        }
    }
}

#[derive(Default)]
pub(super) struct DrawActions {
    pub(super) first_frame: bool,
    pub(super) request_chat: bool,
}

struct DebugUiFrame {
    overlay: Overlay,
    request_chat: bool,
    recreate_renderer: bool,
}

pub(super) struct SceneWindow {
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
    frame_times: FrameTimes,
    animation_redraw_at: Option<Instant>,
    frame_interval: Duration,
    next_frame: Instant,
    dpi: f64,
    occluded: bool,
    focused: bool,
    ready: bool,
    recoveries: u32,
    recovery: PresentationRecovery,
    camera: Option<OrbitCamera>,
    follow_camera: FollowCamera,
    camera_mode: CameraMode,
    controls: HeldControls,
    body_rotation: BodyRotation,
    stop_clear_pending: bool,
    last_camera: Option<scene::Camera>,
    cursor: PhysicalPosition<f64>,
    modifiers: ModifiersState,
    drag: Option<DragState>,
}

impl SceneWindow {
    pub(super) fn new(
        event_loop: &ActiveEventLoop,
        generation: u64,
        activate: bool,
        wake: Arc<SharedWindowState>,
    ) -> Result<Self, SceneWindowError> {
        let attributes = Window::default_attributes()
            .with_title("Radia")
            .with_inner_size(LogicalSize::new(1280, 720))
            .with_min_inner_size(LogicalSize::new(460, 300));
        #[cfg(target_os = "linux")]
        let attributes = {
            let token = event_loop.read_token_from_env();
            reset_activation_token_env();
            if activate {
                match token {
                    Some(token) => attributes.with_activation_token(token),
                    None => attributes,
                }
            } else {
                attributes
            }
        };
        let window = Arc::new(event_loop.create_window(attributes).map_err(SceneWindowError::WindowCreation)?);
        if activate {
            window.focus_window();
        }
        let (egui, platform) = debug_ui(&window);
        let dpi = window.scale_factor();
        let size = window.inner_size();
        let renderer = block_on(Renderer::new(Arc::clone(&window), size.width, size.height)).map_err(SceneWindowError::Renderer)?;
        let info = renderer.adapter_info();
        tracing::info!(backend = ?info.backend, gpu = %info.name, driver = %info.driver, driver_info = %info.driver_info, "scene GPU initialized");
        if !window.has_focus() {
            window.request_user_attention(Some(UserAttentionType::Informational));
        }
        let now = Instant::now();
        let frame_interval = display_refresh_interval(&window);
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
            frame_times: FrameTimes::default(),
            animation_redraw_at: None,
            frame_interval,
            next_frame: now,
            dpi,
            occluded: false,
            focused: true,
            ready: false,
            recoveries: 0,
            recovery: PresentationRecovery::default(),
            camera: None,
            follow_camera: FollowCamera::default(),
            camera_mode: CameraMode::Follow,
            controls: HeldControls::default(),
            body_rotation: BodyRotation::default(),
            stop_clear_pending: false,
            last_camera: None,
            cursor: PhysicalPosition::new(0.0, 0.0),
            modifiers: ModifiersState::empty(),
            drag: None,
        })
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(super) fn matches_window(&self, id: WindowId) -> bool {
        self.window.id() == id
    }

    pub(super) fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        self.platform.on_window_event(&self.window, event).consumed
    }

    pub(super) fn wants_pointer_input(&self) -> bool {
        self.egui.egui_wants_pointer_input()
    }

    pub(super) fn wants_keyboard_input(&self) -> bool {
        self.egui.egui_wants_keyboard_input()
    }

    pub(super) fn handle_scene_input(&mut self, event: &WindowEvent, capture: InputCapture) -> bool {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                let modifiers = modifiers.state();
                if self.modifiers != modifiers {
                    self.modifiers = modifiers;
                    self.dirty = true;
                }
            }
            WindowEvent::Focused(focused) => {
                tracing::info!(focused, "viewport focus changed");
                self.focused = *focused;
                if !focused {
                    self.release_drag(false);
                    self.modifiers = ModifiersState::empty();
                    let now = Instant::now();
                    let (_, seed) = self.agent_pose_and_seed_at(now);
                    let rotation = self.body_rotation.update(seed, 0.0, now);
                    let movement = self.controls.release_and_observe(rotation);
                    self.stop_clear_pending = movement.control_flags & STOP != 0;
                    if let Some(feed) = &mut self.feed {
                        feed.set_movement(movement);
                    }
                }
                self.dirty = true;
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = *position,
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                self.handle_left_click(*state, capture);
            }
            WindowEvent::MouseWheel { delta, .. } if !capture.consumed && !capture.pointer => {
                let clicks = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    MouseScrollDelta::PixelDelta(position) => position.y / (40.0 * self.dpi),
                };
                let zoom = 2.0f64.powf(-clicks / 4.0);
                match self.camera_mode {
                    CameraMode::Follow => {
                        let draw_distance = self.camera.as_ref().map_or(DEFAULT_DRAW_DISTANCE, |camera| camera.draw_distance);
                        self.follow_camera.zoom(zoom, draw_distance);
                    }
                    CameraMode::Free => {
                        if let Some(camera) = &mut self.camera {
                            camera.zoom(zoom);
                        }
                    }
                }
                self.dirty = true;
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
                    && !capture.consumed
                    && !capture.keyboard =>
            {
                self.release_drag(true);
                if self.camera_mode == CameraMode::Free {
                    self.transition_to_follow();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let was_free = self.camera_mode == CameraMode::Free;
                let seated = self.feed.as_mut().is_some_and(SceneFeed::agent_is_seated);
                let Some(changed) = handle_keyboard_input(
                    &mut self.controls,
                    &mut self.camera_mode,
                    &event.logical_key,
                    event.state,
                    event.repeat,
                    capture.consumed || capture.keyboard,
                    seated,
                ) else {
                    return false;
                };
                if changed {
                    self.dirty = true;
                }
                if was_free && self.camera_mode == CameraMode::Follow {
                    self.transition_to_follow();
                }
                if self.camera_mode == CameraMode::Follow {
                    self.release_drag(true);
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) {
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);
        }
    }

    pub(super) fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
    }

    pub(super) fn update_feed(&mut self, incoming: Option<(u64, SceneFeed)>) -> Result<(), scene::ApplyError> {
        if let Some((generation, feed)) = incoming
            && generation == self.generation
        {
            self.feed = Some(feed);
            self.dirty = true;
        }
        if let Some(feed) = &mut self.feed {
            let before = feed.view.version();
            feed.drain()?;
            self.dirty |= before != feed.view.version();
        }
        Ok(())
    }

    pub(super) fn take_retirements(&mut self) -> [Option<Retirement>; 2] {
        [self.renderer.take().map(Renderer::retire), self.retired.take()]
    }

    pub(super) fn take_retired(&mut self) -> Option<Retirement> {
        self.retired.take()
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

    fn handle_left_click(&mut self, state: ElementState, capture: InputCapture) {
        if state == ElementState::Released {
            self.release_drag(true);
            return;
        }
        if capture.consumed || capture.pointer {
            return;
        }

        if !self.modifiers.alt_key() {
            self.start_drag();
            return;
        }

        let point = {
            let (Some(renderer), Some(observation)) = (&self.renderer, self.last_camera) else {
                return;
            };
            let size = self.window.inner_size();
            match renderer.pick(observation, observation.ray([self.cursor.x, self.cursor.y], [size.width, size.height])) {
                Ok(point) => point,
                Err(error) => {
                    tracing::debug!(%error, "scene focus pick exceeded its work limit");
                    return;
                }
            }
        };
        let Some(point) = point else { return };
        self.enter_free_camera();
        if let Some(camera) = &mut self.camera {
            camera.focus(point, Instant::now());
            self.start_drag();
        }
    }

    pub(super) fn drag_camera(&mut self, (dx, dy): (f64, f64)) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        drag.travel[0] += dx.abs() / self.dpi;
        drag.travel[1] += dy.abs() / self.dpi;
        if drag.travel[0] >= 4.0 || drag.travel[1] >= 4.0 {
            self.dirty = true;
            let dy = if drag.travel[1] >= 4.0 { dy } else { 0.0 };
            let width = f64::from(self.window.inner_size().width);
            match self.camera_mode {
                CameraMode::Follow => {
                    let draw_distance = self.camera.as_ref().map_or(DEFAULT_DRAW_DISTANCE, |camera| camera.draw_distance);
                    self.follow_camera.drag([dx, dy], width, self.modifiers.control_key(), self.modifiers.shift_key(), draw_distance);
                }
                CameraMode::Free => {
                    if let Some(camera) = &mut self.camera {
                        camera.drag([dx, dy], width, self.modifiers.control_key(), self.modifiers.shift_key());
                    }
                }
            }
        }
    }

    fn start_drag(&mut self) {
        if self.drag.is_some() {
            return;
        }
        let origin = self.cursor;
        let size = self.window.inner_size();
        let center = PhysicalPosition::new(f64::from(size.width) * 0.5, f64::from(size.height) * 0.5);
        match self.window.set_cursor_grab(CursorGrabMode::Confined) {
            Ok(()) => {
                self.window.set_cursor_visible(false);
                self.drag = Some(DragState { origin, travel: [0.0; 2] });
                if let Err(error) = self.window.set_cursor_position(center) {
                    self.release_drag(false);
                    tracing::warn!(%error, "could not position scene cursor");
                }
            }
            Err(error) => tracing::warn!(%error, "could not capture scene cursor"),
        }
    }

    fn enter_free_camera(&mut self) {
        if self.camera_mode == CameraMode::Free {
            return;
        }
        let Some(mut pose) = self.agent_pose() else { return };
        pose.rotation = self.body_rotation.current();
        let (position, focus) = self.follow_camera.current_position_and_focus(pose, Instant::now());
        let (draw_distance, lod_factor) =
            self.camera.as_ref().map_or((DEFAULT_DRAW_DISTANCE, 1.0), |camera| (camera.draw_distance, camera.lod_factor));
        self.camera = Some(OrbitCamera::from_follow_view(position, focus, draw_distance, lod_factor));
        self.camera_mode.detach_for_focus();
    }

    fn transition_to_follow(&mut self) {
        if let Some(camera) = &self.camera {
            let now = Instant::now();
            let (position, focus) = camera.current_position_and_focus(now);
            self.follow_camera.transition_from(position, focus, now);
        }
        self.camera_mode.return_to_follow();
        self.dirty = true;
    }

    fn ensure_camera(&mut self, pose: Option<world::WorldPose>) {
        if self.camera.is_none()
            && let Some(pose) = pose
        {
            self.camera = Some(OrbitCamera::new(pose.position));
        }
    }

    fn agent_pose(&mut self) -> Option<world::WorldPose> {
        self.feed.as_mut()?.camera_pose()
    }

    fn agent_pose_and_seed_at(&mut self, now: Instant) -> (Option<world::WorldPose>, Option<(u64, math::Quat)>) {
        let Some((pose, seed)) = self.feed.as_mut().and_then(|feed| feed.camera_pose_and_seed_at(now)) else {
            return (None, None);
        };
        (Some(pose), Some(seed))
    }

    fn camera_observation(&mut self, pose: Option<world::WorldPose>, rotation: math::Quat, now: Instant) -> Option<scene::Camera> {
        let viewport = self.window.inner_size().into();
        match self.camera_mode {
            CameraMode::Follow => pose.and_then(|mut pose| {
                pose.rotation = rotation;
                let (draw_distance, lod_factor) =
                    self.camera.as_ref().map_or((DEFAULT_DRAW_DISTANCE, 1.0), |camera| (camera.draw_distance, camera.lod_factor));
                let sit_camera = self.feed.as_ref().and_then(SceneFeed::sit_camera_pose);
                self.follow_camera.follow_with_sit_camera(pose, sit_camera, viewport, draw_distance, lod_factor, now)
            }),
            CameraMode::Free => self.camera.as_ref().and_then(|camera| camera.scene_camera(viewport, now)),
        }
    }

    pub(super) fn draw(&mut self) -> Result<DrawActions, SceneWindowError> {
        let now = Instant::now();
        let (pose, seed) = self.agent_pose_and_seed_at(now);
        self.ensure_camera(pose);
        let seated = self.feed.as_mut().is_some_and(SceneFeed::agent_is_seated);
        if seated {
            self.controls.clear_flying();
        }
        let body_rotation = self.body_rotation.update(seed, if seated { 0.0 } else { self.controls.yaw(self.modifiers.shift_key()) }, now);
        let rotation = if seated { pose.map_or(body_rotation, |pose| pose.rotation) } else { body_rotation };
        let movement: MovementObservation = self.controls.observation(self.modifiers.shift_key(), rotation);
        self.stop_clear_pending = movement.control_flags & STOP != 0;
        let observation = self.camera_observation(pose, rotation, now);
        let agent_pose = pose.map(|mut pose| {
            pose.rotation = rotation;
            pose
        });
        if let Some(feed) = &mut self.feed {
            feed.set_camera(observation);
            feed.set_movement(movement);
        }
        if now < self.next_frame {
            return Ok(DrawActions::default());
        }
        self.dirty = false;
        if !self.ensure_renderer(now)? {
            return Ok(DrawActions::default());
        }
        self.complete_texture_jobs();
        let debug_ui = self.draw_debug_ui(now);
        Ok(self.submit_frame(observation, agent_pose, now, debug_ui))
    }

    fn ensure_renderer(&mut self, now: Instant) -> Result<bool, SceneWindowError> {
        if self.renderer.is_some() {
            return Ok(true);
        }
        match self.recovery.begin_attempt(now) {
            RecoveryAttempt::Start => {}
            RecoveryAttempt::WaitUntil(retry_at) => {
                self.next_frame = retry_at;
                self.dirty = true;
                return Ok(false);
            }
            RecoveryAttempt::Exhausted => return Err(SceneWindowError::RecoveryExhausted),
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
                Ok(true)
            }
            Err(error) => {
                tracing::warn!(%error, attempt = self.recovery.attempts, "scene GPU recovery failed");
                self.schedule_recovery(now);
                Ok(false)
            }
        }
    }

    fn complete_texture_jobs(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            while let Ok(completion) = self.texture_receiver.try_recv() {
                renderer.complete_texture(completion);
            }
        }
    }

    fn draw_debug_ui(&mut self, now: Instant) -> DebugUiFrame {
        let renderer = self.renderer.as_ref().expect("renderer ensured before debug UI");
        let stats = renderer.stats();
        let gpu_timings = renderer.gpu_timings();
        let gpu_timing_supported = renderer.gpu_timing_supported();
        let frame_times = self.frame_times.summary();
        self.dpi = self.window.scale_factor();
        let input = self.platform.take_egui_input(&self.window);
        let mut request_chat = false;
        let mut recreate_renderer = false;
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
                ui.label(format!("Camera: {}", self.camera_mode.label()));
                if let Some(times) = &frame_times {
                    ui.label(format!(
                        "Frametime: p50 {:.2} ms, p95 {:.2} ms",
                        times.p50.as_secs_f64() * 1000.0,
                        times.p95.as_secs_f64() * 1000.0
                    ));
                } else {
                    ui.label("Frametime: no samples");
                }
                ui.label(format!(
                    "Draw submissions: {}, indexed commands: {}, instances: {}",
                    stats.draw_calls, stats.draw_commands, stats.instances
                ));
                ui.collapsing("Diagnostics", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(DragValue::new(&mut self.texture_soft_cap).speed(16).suffix(" MiB"))
                            .on_hover_text("Soft GPU backstop. Textures used by the current frame stay resident, even above this value.");
                        ui.label("Texture soft cap");
                    });
                    ui.label(format!("CPU prepare: {:.2} ms", stats.prepare_ms));
                    ui.label(format!(
                        "Prepare work: layouts {}, items updated {}, faces patched {}, blended sorts {}",
                        stats.layouts, stats.items_updated, stats.faces_patched, stats.blended_sorts
                    ));
                    ui.label(format!(
                        "Animated texture faces: {} visited, {} changed",
                        stats.animated_faces_visited, stats.animated_faces_changed
                    ));
                    if let Some((scene_ms, diagnostics_ms)) = gpu_timings {
                        ui.label(format!("GPU passes: scene {:.2} ms, diagnostics {:.2} ms", scene_ms, diagnostics_ms));
                    } else if gpu_timing_supported {
                        ui.label("GPU pass timings: collecting");
                    }
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
                        let reason = match stats.omission_reason {
                            Some(render::OmissionReason::InstanceBufferBudget) => {
                                "GPU instance buffer budget; nearest and largest objects kept first"
                            }
                            Some(render::OmissionReason::MeshMemoryBudget) => {
                                "GPU mesh memory budget; nearest and largest meshes kept first"
                            }
                            Some(render::OmissionReason::CombinedBudgets) => {
                                "GPU mesh and instance budgets; nearest and largest content kept first"
                            }
                            None => "unknown renderer limit",
                        };
                        ui.label(format!("Faces omitted: {} ({reason})", stats.omitted_faces));
                    }
                    if stats.mesh_budget_blocked > 0 {
                        ui.label(format!("Meshes blocked by GPU memory budget: {}", stats.mesh_budget_blocked));
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
                        if let Some(cache) = feed.assets.cache_stats() {
                            ui.label(format!(
                                "Asset cache: {:.1}/{:.1} MiB, protected {:.1} MiB",
                                cache.size_bytes as f64 / 1048576.0,
                                cache.size_limit_bytes as f64 / 1048576.0,
                                cache.protected_bytes as f64 / 1048576.0
                            ));
                            ui.label(format!(
                                "Cached: {} textures, {} PBR, {} legacy, {} environments, {} meshes",
                                cache.entries_by_kind.textures,
                                cache.entries_by_kind.pbr_materials,
                                cache.entries_by_kind.legacy_materials,
                                cache.entries_by_kind.environments,
                                cache.entries_by_kind.meshes
                            ));
                            let reasons = cache
                                .recent_evictions
                                .iter()
                                .filter_map(|reason| {
                                    reason.map(|reason| match reason {
                                        assets::cache::CacheEvictionReason::Infrequent => "infrequent",
                                        assets::cache::CacheEvictionReason::Stale => "stale",
                                        assets::cache::CacheEvictionReason::LowReconstructionCost => "low decode cost",
                                        assets::cache::CacheEvictionReason::LeastRecentlyUsed => "least recent",
                                        assets::cache::CacheEvictionReason::LowUtility => "low utility",
                                    })
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            if !reasons.is_empty() {
                                ui.label(format!("Recent evictions: {reasons}"));
                            }
                        }
                    }
                    if self.recoveries > 0 {
                        ui.label(format!("GPU recreations: {}", self.recoveries));
                    }
                    recreate_renderer = ui.button("Recreate GPU resources").clicked();
                });
                request_chat = ui.button("Nearby chat (F6)").clicked();
            });
        });
        self.next_ui = output.viewport_output.get(&ViewportId::ROOT).and_then(|viewport| now.checked_add(viewport.repaint_delay));
        self.platform.handle_platform_output(&self.window, output.platform_output);
        if self.drag.is_some() {
            self.window.set_cursor_visible(false);
        }
        DebugUiFrame {
            overlay: Overlay {
                primitives: self.egui.tessellate(output.shapes, output.pixels_per_point),
                textures: output.textures_delta,
                pixels_per_point: output.pixels_per_point,
            },
            request_chat,
            recreate_renderer,
        }
    }

    fn submit_frame(
        &mut self,
        observation: Option<scene::Camera>,
        agent_pose: Option<world::WorldPose>,
        now: Instant,
        ui: DebugUiFrame,
    ) -> DrawActions {
        let scheduled_deadline = self.next_frame;
        if let Some(feed) = &mut self.feed {
            feed.refresh_motion_circuits(now);
        }
        let scene = self.feed.as_ref().zip(observation).map(|(feed, camera)| (&feed.view, camera));
        let avatar_pose = self.feed.as_ref().zip(agent_pose).map(|(feed, pose)| scene::AgentPose::new(feed.agent_id(), pose));
        let water_height = self.feed.as_ref().and_then(|feed| feed.water_height);
        let result = {
            let renderer = self.renderer.as_mut().expect("renderer ensured before frame submission");
            renderer.set_texture_soft_cap(self.texture_soft_cap);
            renderer.draw(ui.overlay, scene, avatar_pose, water_height, || self.window.pre_present_notify())
        };
        self.dispatch_texture_jobs();
        let completed = Instant::now();
        let frame_work = completed.saturating_duration_since(now);
        match result {
            Ok(presented) => {
                let active_motion = self.controls.active() || self.stop_clear_pending;
                self.dirty |= !presented
                    || active_motion
                    || self.feed.as_mut().is_some_and(|feed| feed.agent_motion_active(Instant::now()))
                    || (self.camera_mode == CameraMode::Follow
                        && agent_pose.is_some_and(|agent_pose| self.follow_camera.moving(agent_pose, Instant::now())))
                    || (self.camera_mode == CameraMode::Free && self.camera.as_ref().is_some_and(|camera| camera.moving(Instant::now())));
                if presented {
                    self.frame_times.record(frame_work);
                    self.last_camera = observation;
                    self.recovery.presented();
                    if self.dirty {
                        self.next_frame = next_frame_deadline(scheduled_deadline, completed, self.frame_interval);
                    } else {
                        self.next_frame = completed;
                    }
                } else {
                    self.next_frame = next_frame_deadline(scheduled_deadline, completed, self.frame_interval);
                }
                let first_frame = presented && !self.ready;
                self.ready |= presented;
                if ui.recreate_renderer {
                    self.retire_renderer();
                    self.schedule_recovery(now);
                }
                DrawActions { first_frame, request_chat: ui.request_chat }
            }
            Err(error) => {
                tracing::warn!(%error, "scene presentation failed; scheduling GPU recovery");
                self.retire_renderer();
                self.schedule_recovery(now);
                DrawActions { first_frame: false, request_chat: ui.request_chat }
            }
        }
    }

    fn dispatch_texture_jobs(&mut self) {
        if let Some(feed) = &self.feed {
            let Some(renderer) = &mut self.renderer else {
                return;
            };
            while let Some(job) = renderer.next_texture_job() {
                let sender = self.texture_sender.clone();
                let wake = Arc::clone(&self.wake);
                let span = tracing::Span::current();
                let work = feed.runtime.spawn_blocking(move || span.in_scope(|| job.run(CompressionQuality::Fast)));
                drop(
                    feed.runtime.spawn(
                        async move {
                            match work.await {
                                Ok(completion) => match sender.try_send(completion) {
                                    Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
                                    Err(mpsc::TrySendError::Full(_)) => tracing::warn!("texture completion queue is full"),
                                },
                                Err(error) => tracing::error!(%error, "texture worker stopped unexpectedly"),
                            }
                            wake.wake();
                        }
                        .in_current_span(),
                    ),
                );
            }
        }
    }

    pub(super) fn poll_renderer(&mut self, now: Instant) -> bool {
        let Some(renderer) = &mut self.renderer else {
            self.animation_redraw_at = None;
            return false;
        };
        match renderer.poll() {
            Ok(progress) => {
                self.animation_redraw_at = progress.redraw_at;
                self.dirty |= progress.redraw;
                progress.pending
            }
            Err(error) => {
                self.animation_redraw_at = None;
                tracing::warn!(%error, "scene GPU failed; scheduling recovery");
                self.retire_renderer();
                self.schedule_recovery(now);
                true
            }
        }
    }

    fn schedule_recovery(&mut self, now: Instant) {
        self.next_frame = self.recovery.schedule_retry(now);
        self.dirty = true;
    }

    pub(super) fn schedule_redraw(&mut self, event_loop: &ActiveEventLoop, pending: bool, can_redraw: bool) {
        if self.occluded || self.window.inner_size().width == 0 || self.window.inner_size().height == 0 {
            event_loop.set_control_flow(if pending {
                ControlFlow::WaitUntil(Instant::now() + self.frame_interval)
            } else {
                ControlFlow::Wait
            });
            return;
        }
        let display_interval = display_refresh_interval(&self.window);
        self.frame_interval = redraw_interval(display_interval, self.focused, self.controls.active() || self.stop_clear_pending);
        let now = Instant::now();
        if self.next_ui.is_some_and(|at| now >= at) {
            self.next_ui = None;
            self.dirty = true;
        }
        if self.dirty && now >= self.next_frame && can_redraw {
            self.window.request_redraw();
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            let redraw = (self.dirty && can_redraw).then_some(self.next_frame.max(now + Duration::from_millis(1)));
            let poll = pending.then_some(now + self.frame_interval);
            event_loop.set_control_flow(match redraw.into_iter().chain(poll).chain(self.animation_redraw_at).chain(self.next_ui).min() {
                Some(at) => ControlFlow::WaitUntil(at),
                None => ControlFlow::Wait,
            });
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

struct DragState {
    origin: PhysicalPosition<f64>,
    travel: [f64; 2],
}

#[derive(Debug, PartialEq, Eq)]
enum RecoveryAttempt {
    Start,
    WaitUntil(Instant),
    Exhausted,
}

#[derive(Default)]
struct PresentationRecovery {
    attempts: u32,
    retry_at: Option<Instant>,
}

impl PresentationRecovery {
    fn begin_attempt(&mut self, now: Instant) -> RecoveryAttempt {
        if let Some(retry_at) = self.retry_at
            && now < retry_at
        {
            return RecoveryAttempt::WaitUntil(retry_at);
        }
        if self.attempts == MAX_RECOVERIES {
            return RecoveryAttempt::Exhausted;
        }
        self.attempts += 1;
        self.retry_at = None;
        RecoveryAttempt::Start
    }

    fn schedule_retry(&mut self, now: Instant) -> Instant {
        let retry_at = now + RECOVERY_DELAY;
        self.retry_at = Some(retry_at);
        retry_at
    }

    fn presented(&mut self) {
        self.attempts = 0;
        self.retry_at = None;
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
    use std::{
        error::Error,
        time::{Duration, Instant},
    };

    use super::{PresentationRecovery, RECOVERY_DELAY, RecoveryAttempt, SceneWindowError};

    #[test]
    fn presentation_resets_recovery_limit() {
        let mut recovery = PresentationRecovery::default();
        let mut now = Instant::now();
        for _ in 0..6 {
            assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start);
            recovery.presented();
        }
        for _ in 0..3 {
            assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start);
            now = recovery.schedule_retry(now);
        }
        assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Exhausted, "three attempts without a presented frame exhaust recovery");
        recovery.presented();
        assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start, "a presented frame makes recovery available again");
    }

    #[test]
    fn recovery_retries_after_delay() {
        let mut recovery = PresentationRecovery::default();
        let now = Instant::now();

        assert_eq!(recovery.begin_attempt(now), RecoveryAttempt::Start);
        let retry_at = recovery.schedule_retry(now);
        assert_eq!(retry_at, now + RECOVERY_DELAY);
        assert_eq!(recovery.begin_attempt(retry_at - Duration::from_nanos(1)), RecoveryAttempt::WaitUntil(retry_at));
        assert_eq!(recovery.begin_attempt(retry_at), RecoveryAttempt::Start);
    }

    #[test]
    fn recovery_error_keeps_user_message() {
        assert_eq!(SceneWindowError::RecoveryExhausted.to_string(), "The scene GPU could not recover after three attempts.");
    }

    #[test]
    fn renderer_creation_error_has_source() {
        let error = SceneWindowError::Renderer(render::Error::UnsupportedSurface);
        let source = Error::source(&error).expect("renderer creation error must retain its source");
        assert!(source.downcast_ref::<render::Error>().is_some());
    }
}
