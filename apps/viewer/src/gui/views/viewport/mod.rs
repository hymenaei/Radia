mod timing;
mod window;

use std::{
    error::Error,
    fmt, io,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use futures::channel::mpsc::{Receiver, Sender, channel};
use render::Retirement;
use render::scene;
use tokio::runtime::Handle;
use tokio::sync::watch;
use winit::application::ApplicationHandler;
#[cfg(target_os = "linux")]
use winit::platform::wayland::EventLoopBuilderExtWayland;
#[cfg(target_os = "windows")]
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::{
    event::{DeviceEvent, DeviceId, ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    window::WindowId,
};

use self::window::{MAX_RECOVERIES, SceneWindow, SceneWindowError};
use crate::{
    agent_movement::{MovementObservation, ScriptControlGrabs},
    scene_feed::SceneFeed,
    session::{AgentMotionSample, SitResponseState},
};

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

#[derive(Debug)]
pub(crate) enum ViewportError {
    SceneWindow(SceneWindowError),
    ScenePublication(render::scene::ApplyError),
    WorkerStart(io::Error),
    WorkerStopped,
    WorkerUnavailable,
}

impl fmt::Display for ViewportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SceneWindow(error) => fmt::Display::fmt(error, f),
            Self::ScenePublication(error) => write!(f, "Scene publication failed: {error:?}"),
            Self::WorkerStart(error) => fmt::Display::fmt(error, f),
            Self::WorkerStopped => f.write_str("The scene window stopped unexpectedly."),
            Self::WorkerUnavailable => f.write_str("The scene window worker is unavailable."),
        }
    }
}

impl Error for ViewportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SceneWindow(error) => Some(error),
            Self::ScenePublication(error) => Some(error),
            Self::WorkerStart(error) => Some(error),
            Self::WorkerStopped | Self::WorkerUnavailable => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ViewportEvent {
    Ready(u64),
    Chat(u64),
    Closed(u64),
    Failed(u64, ViewportError),
}

#[derive(Clone, Copy)]
struct InputCapture {
    consumed: bool,
    pointer: bool,
    keyboard: bool,
}

#[derive(Clone, Copy, Default)]
struct RequestedScene {
    generation: u64,
    visible: bool,
    activate: bool,
}

impl RequestedScene {
    fn show(&mut self, generation: u64, activate: bool) -> bool {
        if generation > self.generation {
            *self = Self { generation, visible: true, activate };
            true
        } else {
            false
        }
    }

    fn clear(&mut self, generation: u64) {
        if self.generation == generation {
            self.visible = false;
            self.activate = false;
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
            let _span = tracing::info_span!("viewer.viewport").entered();
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

    pub(crate) fn show(&self, generation: u64, activate: bool) -> bool {
        {
            let mut requested = self.shared.requested.lock().unwrap_or_else(PoisonError::into_inner);
            if !requested.show(generation, activate) {
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
        movement: watch::Sender<Option<MovementObservation>>,
        agent_motion: watch::Receiver<Option<AgentMotionSample>>,
        sit_response: watch::Receiver<Option<SitResponseState>>,
        sit_control_grabs: watch::Receiver<ScriptControlGrabs>,
        runtime: Handle,
    ) {
        let requested = self.shared.requested.lock().unwrap_or_else(PoisonError::into_inner);
        if requested.generation == generation && requested.visible {
            let shared = Arc::clone(&self.shared);
            let feed =
                SceneFeed::start(sources, agent_id, camera, movement, agent_motion, sit_response, sit_control_grabs, runtime, move || {
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
            self.retired.extend(active.take_retirements().into_iter().flatten());
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
            match SceneWindow::new(event_loop, requested.generation, requested.activate, Arc::clone(&self.shared)) {
                Ok(window) => self.active = Some(window),
                Err(error) => self.publish(ViewportEvent::Failed(requested.generation, ViewportError::SceneWindow(error)), event_loop),
            }
        }
        let incoming = self.shared.feed.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(active) = &mut self.active
            && let Err(error) = active.update_feed(incoming)
        {
            let generation = active.generation();
            self.retire_active();
            self.publish(ViewportEvent::Failed(generation, ViewportError::ScenePublication(error)), event_loop);
        }
    }

    fn request_chat(&mut self, generation: u64) {
        if !self.shared.chat_pending.swap(true, Ordering::AcqRel)
            && let Err(error) = self.events.try_send(ViewportEvent::Chat(generation))
        {
            self.shared.chat_pending.store(false, Ordering::Release);
            if error.is_disconnected() {
                tracing::trace!(%error, "nearby chat request receiver was closed");
            } else {
                tracing::warn!(%error, "nearby chat request queue is full");
            }
        }
    }

    fn redraw_active(&mut self, event_loop: &ActiveEventLoop) {
        let Some(active) = &mut self.active else {
            return;
        };
        let generation = active.generation();
        match active.draw() {
            Ok(actions) => {
                if actions.first_frame {
                    self.publish(ViewportEvent::Ready(generation), event_loop);
                }
                if actions.request_chat {
                    self.request_chat(generation);
                }
            }
            Err(error) => {
                self.retire_active();
                self.publish(ViewportEvent::Failed(generation, ViewportError::SceneWindow(error)), event_loop);
            }
        }
    }

    fn schedule_redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(active) = &mut self.active else {
            event_loop.set_control_flow(if self.retired.is_empty() {
                ControlFlow::Wait
            } else {
                ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL)
            });
            return;
        };
        if let Some(retired) = active.take_retired() {
            self.retired.push(retired);
        }
        let renderer_pending = active.poll_renderer(Instant::now());
        let pending = !self.retired.is_empty() || renderer_pending;
        active.schedule_redraw(event_loop, pending, self.retired.len() < MAX_RECOVERIES as usize);
    }
}

impl ApplicationHandler for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.synchronize(event_loop);
        if let Some(active) = &mut self.active {
            active.mark_dirty();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.shared.wake_pending.store(false, Ordering::Release);
        self.synchronize(event_loop);
        if let Some(active) = &mut self.active {
            active.mark_dirty();
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && let Some(active) = &mut self.active
        {
            active.drag_camera(delta);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let capture = {
            let Some(active) = &mut self.active else {
                return;
            };
            if !active.matches_window(id) {
                return;
            }
            active.mark_dirty();
            InputCapture {
                consumed: active.on_window_event(&event),
                pointer: active.wants_pointer_input(),
                keyboard: active.wants_keyboard_input(),
            }
        };
        if self.active.as_mut().is_some_and(|active| active.handle_scene_input(&event, capture)) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                let generation = self.active.as_ref().map(SceneWindow::generation);
                self.retire_active();
                if let Some(generation) = generation {
                    self.publish(ViewportEvent::Closed(generation), event_loop);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(active) = &mut self.active {
                    active.resize(size.width, size.height);
                }
            }
            WindowEvent::Occluded(occluded) => {
                if let Some(active) = &mut self.active {
                    active.set_occluded(occluded);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat && event.logical_key == Key::Named(NamedKey::F6) =>
            {
                if let Some(generation) = self.active.as_ref().map(SceneWindow::generation) {
                    self.request_chat(generation);
                }
            }
            WindowEvent::RedrawRequested => self.redraw_active(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_retired();
        self.synchronize(event_loop);
        self.schedule_redraw(event_loop);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::channel::mpsc::channel;

    use super::{RequestedScene, SharedWindowState, ViewportEvent, WindowApp};

    #[test]
    fn rejected_chat_requests_can_retry() {
        let (events, mut incoming) = channel(0);
        let mut app =
            WindowApp { shared: Arc::new(SharedWindowState::default()), events, active: None, retired: Vec::new(), generation: 1 };
        app.events.try_send(ViewportEvent::Ready(1)).unwrap();
        app.request_chat(1);
        std::assert_matches!(incoming.try_recv(), Ok(ViewportEvent::Ready(1)));
        app.request_chat(1);
        assert!(matches!(incoming.try_recv(), Ok(ViewportEvent::Chat(1))), "a rejected request must not suppress a later request");
        app.request_chat(1);
        assert!(incoming.try_recv().is_err(), "an accepted request must coalesce until received");
    }

    #[test]
    fn requests_cannot_revive_cancelled_sessions() {
        let mut requested = RequestedScene::default();
        for generation in 1..=100 {
            requested.show(generation, false);
        }
        requested.clear(99);
        assert_eq!(requested.generation, 100);
        assert!(requested.visible, "late cancellation must leave the current session visible");
        requested.clear(100);
        requested.show(100, false);
        requested.show(99, false);
        assert!(!requested.visible, "neither repeated nor stale startup may revive a cancelled scene");
        requested.show(101, true);
        assert!(requested.visible);
        assert!(requested.activate, "the startup request carries the focus intent to winit");
        assert_eq!(requested.generation, 101);
        requested.clear(101);
        assert!(!requested.activate, "a cancelled startup must not leave a stale activation request");
    }
}
