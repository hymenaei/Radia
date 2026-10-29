use crate::{
    agent_movement::MovementObservation,
    asset_coordinator::AssetHandle,
    gui::{
        AppLifecycle,
        components::window_chrome,
        login_options, service_runtime, shutdown,
        views::viewport::{Viewport, ViewportError},
    },
    scene_feed::SceneSources,
    session::{AgentMotionSample, EnvironmentInbox},
    world_reader::WorldReader,
};
use futures::{StreamExt, future::BoxFuture};
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, BackgroundExecutor, Entity, Render, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowId, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{Root, Theme};
use render::scene::Camera;
use tokio::sync::watch;
use uuid::Uuid;

pub(super) struct State {
    main: MainWindow,
    chat_window: Option<AnyWindowHandle>,
    viewport: Option<Viewport>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MainWindow {
    Login,
    Starting(u64),
    Scene(u64),
}

impl MainWindow {
    fn blocks_start(self, generation: u64) -> bool {
        matches!(self, Self::Starting(active) | Self::Scene(active) if active >= generation)
    }

    fn is_current(self, generation: u64) -> bool {
        matches!(self, Self::Starting(active) | Self::Scene(active) if active == generation)
    }
}

impl State {
    pub(super) fn new() -> Self {
        Self { main: MainWindow::Login, chat_window: None, viewport: None }
    }

    pub(super) fn window_closed(&mut self, closed_window_id: WindowId) -> bool {
        if self.chat_window.is_some_and(|window| window.window_id() == closed_window_id) {
            self.chat_window = None;
        }
        matches!(self.main, MainWindow::Scene(_))
    }

    pub(super) fn shutdown(&mut self, executor: BackgroundExecutor) -> BoxFuture<'static, ()> {
        let viewport = self.viewport.take();
        let worker = executor.spawn(async move {
            if let Some(viewport) = viewport {
                viewport.shutdown();
            }
        });
        Box::pin(worker)
    }
}

pub(crate) fn start_session(
    generation: u64,
    reader: &WorldReader,
    environments: EnvironmentInbox,
    assets: &AssetHandle,
    agent_id: Uuid,
    camera: watch::Sender<Option<Camera>>,
    movement: watch::Sender<Option<MovementObservation>>,
    agent_motion: watch::Receiver<Option<AgentMotionSample>>,
    cx: &mut App,
) {
    if start_scene(generation, cx) {
        connect_world(
            generation,
            SceneSources { reader: reader.clone(), environments, assets: assets.clone() },
            agent_id,
            camera,
            movement,
            agent_motion,
            cx,
        );
    }
}

pub(crate) fn end_session(generation: u64, cx: &mut App) {
    if !cx.has_global::<AppLifecycle>() {
        return;
    }
    let lifecycle = cx.global_mut::<AppLifecycle>();
    if !lifecycle.scene.main.is_current(generation) {
        return;
    }
    lifecycle.scene.main = MainWindow::Login;
    if let Some(viewport) = &lifecycle.scene.viewport {
        viewport.clear(generation);
    }
    cx.defer(show_login);
}

pub(crate) fn login_content(_: bool, login_body: AnyElement, _: AnyElement) -> AnyElement {
    login_body
}

fn start_scene(generation: u64, cx: &mut App) -> bool {
    if !cx.has_global::<AppLifecycle>() || cx.global::<AppLifecycle>().scene.main.blocks_start(generation) {
        return false;
    }
    let login_window = cx.global::<AppLifecycle>().login_window;
    let activate = login_window.is_some_and(|window| window.update(cx, |_, window, _| window.is_window_active()).unwrap_or(false));
    if cx.global::<AppLifecycle>().scene.viewport.is_none() {
        match Viewport::start() {
            Ok((viewport, mut events)) => {
                cx.global_mut::<AppLifecycle>().scene.viewport = Some(viewport);
                cx.spawn(async move |cx| {
                    while let Some(event) = events.next().await {
                        cx.update(|cx| viewport_event(event, cx));
                    }
                    cx.update(|cx| {
                        let main = cx.global::<AppLifecycle>().scene.main;
                        if let MainWindow::Starting(generation) | MainWindow::Scene(generation) = main {
                            scene_failed(generation, ViewportError::WorkerStopped, cx);
                        }
                    });
                })
                .detach();
            }
            Err(error) => {
                cx.global_mut::<AppLifecycle>().scene.main = MainWindow::Starting(generation);
                cx.defer(move |cx| scene_failed(generation, ViewportError::WorkerStart(error), cx));
                return false;
            }
        }
    }
    let lifecycle = cx.global_mut::<AppLifecycle>();
    lifecycle.scene.main = MainWindow::Starting(generation);
    if !lifecycle.scene.viewport.as_ref().is_some_and(|viewport| viewport.show(generation, activate)) {
        cx.defer(move |cx| scene_failed(generation, ViewportError::WorkerUnavailable, cx));
        return false;
    }
    true
}

fn connect_world(
    generation: u64,
    sources: SceneSources,
    agent_id: Uuid,
    camera: watch::Sender<Option<Camera>>,
    movement: watch::Sender<Option<MovementObservation>>,
    agent_motion: watch::Receiver<Option<AgentMotionSample>>,
    cx: &mut App,
) {
    let lifecycle = cx.global::<AppLifecycle>();
    if !lifecycle.quitting
        && lifecycle.scene.main.is_current(generation)
        && let Some(viewport) = &lifecycle.scene.viewport
    {
        viewport.connect_world(generation, sources, agent_id, camera, movement, agent_motion, service_runtime(cx));
    }
}

fn viewport_event(event: crate::gui::views::viewport::ViewportEvent, cx: &mut App) {
    use crate::gui::views::viewport::ViewportEvent;

    let main = cx.global::<AppLifecycle>().scene.main;
    if matches!(event, ViewportEvent::Chat(_))
        && let Some(viewport) = &cx.global::<AppLifecycle>().scene.viewport
    {
        viewport.chat_received();
    }
    match event {
        ViewportEvent::Ready(generation) if main == MainWindow::Starting(generation) => {
            cx.global_mut::<AppLifecycle>().scene.main = MainWindow::Scene(generation);
            show_chat(false, cx);
            if let Some(window) = cx.global_mut::<AppLifecycle>().login_window.take()
                && window.update(cx, |_, window, _| window.remove_window()).is_err()
            {
                tracing::trace!("login window was already unavailable after scene startup");
            }
        }
        ViewportEvent::Chat(generation) if main == MainWindow::Scene(generation) => show_chat(true, cx),
        ViewportEvent::Closed(generation) if main.is_current(generation) => shutdown(cx),
        ViewportEvent::Failed(generation, error) => scene_failed(generation, error, cx),
        _ => {}
    }
}

fn scene_failed(generation: u64, error: ViewportError, cx: &mut App) {
    let lifecycle = cx.global::<AppLifecycle>();
    if lifecycle.quitting || !lifecycle.scene.main.is_current(generation) {
        return;
    }
    let controller = lifecycle.controller.clone();
    if let Some(controller) = controller {
        controller.update(cx, |login, cx| {
            tracing::error!(%error, "scene renderer stopped");
            login.end_session("Unable to display the world.".into(), cx);
        });
    }
}

fn show_login(cx: &mut App) {
    let lifecycle = cx.global::<AppLifecycle>();
    if lifecycle.quitting {
        return;
    }
    if let Some(handle) = lifecycle.login_window {
        if handle.update(cx, |_, window, _| window.activate_window()).is_err() {
            tracing::trace!("login window was already unavailable during activation");
        }
    } else {
        let Some(controller) = lifecycle.controller.clone() else {
            return;
        };
        match cx.open_window(login_options(cx), move |window, cx| {
            controller.update(cx, |login, cx| login.attach_window(window, cx));
            cx.new(|cx| Root::new(controller, window, cx))
        }) {
            Ok(handle) => cx.global_mut::<AppLifecycle>().login_window = Some(handle.into()),
            Err(error) => {
                tracing::error!(%error, "could not reopen login window");
                return;
            }
        }
    }
    if let Some(chat) = cx.global_mut::<AppLifecycle>().scene.chat_window.take()
        && chat.update(cx, |_, window, _| window.remove_window()).is_err()
    {
        tracing::trace!("chat window was already unavailable during removal");
    }
}

fn show_chat(focus: bool, cx: &mut App) {
    let lifecycle = cx.global::<AppLifecycle>();
    if lifecycle.quitting {
        return;
    }
    if let Some(window) = lifecycle.scene.chat_window {
        if focus && window.update(cx, |_, window, _| window.activate_window()).is_err() {
            tracing::trace!("nearby chat window was already unavailable during activation");
        }
        return;
    }
    let Some(controller) = &lifecycle.controller else {
        return;
    };
    let chat = controller.read(cx).chat_view();
    let minimum = size(px(460.), px(300.));
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Nearby chat".into()),
            appears_transparent: window_chrome::appears_transparent(),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(super::window_geometry::restore_bounds(
            "chat",
            None,
            size(px(640.), px(520.)),
            minimum,
            cx,
        ))),
        window_min_size: Some(minimum),
        focus,
        ..Default::default()
    };
    match cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| ChatWindow::new(chat, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(window) => cx.global_mut::<AppLifecycle>().scene.chat_window = Some(window.into()),
        Err(error) => tracing::error!(%error, "could not open nearby chat window"),
    }
}

struct ChatWindow {
    chat: Entity<super::views::chat::ChatScreen>,
    _geometry: Subscription,
}

impl ChatWindow {
    fn new(chat: Entity<super::views::chat::ChatScreen>, window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        Self { chat, _geometry: super::window_geometry::observe("chat", window, cx) }
    }
}

impl Render for ChatWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let theme = Theme::global(cx).colors;
        let titlebar = window_chrome::titlebar(window, Some("Nearby Chat"), theme);
        let content = div().size_full().flex().flex_col().bg(theme.background).text_color(theme.foreground);
        let content = if let Some(titlebar) = titlebar { content.child(titlebar) } else { content };
        content.child(div().flex_1().min_h_0().child(self.chat.clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{MainWindow, start_scene, viewport_event};
    use crate::gui::{
        AppLifecycle, install_lifecycle,
        views::{
            login::LoginScreen,
            viewport::{ViewportError, ViewportEvent},
        },
    };

    #[gpui::test]
    fn handoff_preserves_window_lifetimes(cx: &mut gpui::TestAppContext) {
        let session = Arc::new(Mutex::new(None));
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            install_lifecycle(Arc::clone(&session), cx);
        });
        let (controller, visual) = cx.add_window_view(|window, cx| LoginScreen::new(window, cx, session));
        let login_id = visual.update(|window, cx| {
            let lifecycle = cx.global_mut::<AppLifecycle>();
            lifecycle.controller = Some(controller.clone());
            lifecycle.login_window = Some(window.window_handle());
            lifecycle.scene.main = MainWindow::Starting(2);
            window.window_handle().window_id()
        });
        cx.update(|cx| {
            viewport_event(ViewportEvent::Ready(1), cx);
            assert_eq!(cx.global::<AppLifecycle>().scene.main, MainWindow::Starting(2));
            assert!(!start_scene(2, cx), "repeated readiness must not restart a pending scene");
            viewport_event(ViewportEvent::Ready(2), cx);
            let lifecycle = cx.global::<AppLifecycle>();
            assert_eq!(lifecycle.scene.main, MainWindow::Scene(2));
            assert!(lifecycle.login_window.is_none());
            assert_ne!(lifecycle.scene.chat_window.unwrap().window_id(), login_id, "chat must have its own native window");
            assert!(!start_scene(2, cx), "repeated readiness must not restart the active scene");
            assert!(!start_scene(1, cx), "older readiness must not replace the active scene");
            assert_eq!(cx.global::<AppLifecycle>().scene.main, MainWindow::Scene(2));
            viewport_event(ViewportEvent::Closed(1), cx);
            viewport_event(ViewportEvent::Failed(1, ViewportError::ScenePublication(render::scene::ApplyError::StaleSession)), cx);
            assert!(!cx.global::<AppLifecycle>().quitting);
            let chat = cx.global::<AppLifecycle>().scene.chat_window.unwrap();
            chat.update(cx, |_, window, _| window.remove_window()).unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let lifecycle = cx.global::<AppLifecycle>();
            assert!(lifecycle.login_window.is_none());
            assert!(lifecycle.scene.chat_window.is_none());
            assert_eq!(lifecycle.scene.main, MainWindow::Scene(2));
            assert!(!lifecycle.quitting, "closing the last GPUI window must leave the scene and session alive");
            viewport_event(ViewportEvent::Chat(2), cx);
            assert!(cx.global::<AppLifecycle>().scene.chat_window.is_some(), "chat must reopen without replacing the session controller");
            assert_eq!(cx.global::<AppLifecycle>().controller.as_ref().unwrap().entity_id(), controller.entity_id());
            viewport_event(ViewportEvent::Closed(2), cx);
            assert!(cx.global::<AppLifecycle>().quitting, "closing the current scene owns shutdown");
        });
        cx.run_until_parked();
    }
}
