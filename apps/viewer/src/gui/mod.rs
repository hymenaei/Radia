#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) mod camera;
mod chat;
mod components;
mod login;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod viewport;
mod window_geometry;

use std::sync::{Arc, Mutex, PoisonError};

use gpui::{
    AnyWindowHandle, App, AppContext, ColorExt, Entity, Global, QuitMode, Subscription, TitlebarOptions, WindowBounds, WindowOptions, px,
    rgb, rgb_to_hsla, size,
};
use gpui_component::{Root, Theme, ThemeMode};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use scene::Camera;
use tokio::runtime::Handle;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use tokio::sync::watch;
use uuid::Uuid;

use crate::{gui::login::LoginScreen, gui::window_geometry::restore_bounds};

struct AppLifecycle {
    _window_closed: Subscription,
    controller: Option<Entity<LoginScreen>>,
    login_window: Option<AnyWindowHandle>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    chat_window: Option<AnyWindowHandle>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    main: MainWindow,
    quitting: bool,
    session: Arc<Mutex<Option<network::SessionHandle>>>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    viewport: Option<crate::gui::viewport::Viewport>,
}

impl Global for AppLifecycle {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
enum MainWindow {
    Login,
    Starting(u64),
    Scene(u64),
}

struct ServiceRuntime(Handle);

impl Global for ServiceRuntime {}

pub(crate) fn service_runtime(cx: &App) -> Handle {
    cx.global::<ServiceRuntime>().0.clone()
}

pub fn init(cx: &mut App, services: Handle) {
    let shutdown_session: Arc<Mutex<Option<network::SessionHandle>>> = Arc::new(Mutex::new(None));
    cx.set_global(ServiceRuntime(services));
    cx.set_quit_mode(QuitMode::Explicit);

    gpui_component::init(cx);
    set_dark_theme(cx);

    install_lifecycle(Arc::clone(&shutdown_session), cx);

    let main_window = cx
        .open_window(login_options(cx), move |window, cx| {
            cx.new(|cx| {
                let login = cx.new(|cx| LoginScreen::new(window, cx, shutdown_session));
                cx.global_mut::<AppLifecycle>().controller = Some(login.clone());
                Root::new(login, window, cx)
            })
        })
        .expect("failed to open the main window");
    cx.global_mut::<AppLifecycle>().login_window = Some(main_window.into());
    cx.activate(true);
}

fn install_lifecycle(session: Arc<Mutex<Option<network::SessionHandle>>>, cx: &mut App) {
    let window_closed = cx.on_window_closed(|cx, closed_window_id| {
        let lifecycle = cx.global_mut::<AppLifecycle>();
        let closing_main = lifecycle.login_window.is_some_and(|window| window.window_id() == closed_window_id);
        if closing_main {
            lifecycle.login_window = None;
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            if lifecycle.chat_window.is_some_and(|window| window.window_id() == closed_window_id) {
                lifecycle.chat_window = None;
            }
            if matches!(lifecycle.main, MainWindow::Scene(_)) {
                return;
            }
        }
        if should_shutdown(closing_main, cx.windows().len()) {
            shutdown(cx);
        }
    });
    cx.set_global(AppLifecycle {
        _window_closed: window_closed,
        controller: None,
        login_window: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        chat_window: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        main: MainWindow::Login,
        quitting: false,
        session,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        viewport: None,
    });
}

fn login_options(cx: &App) -> WindowOptions {
    let minimum = size(px(460.), px(420.));
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Radia".into()),
            appears_transparent: cfg!(target_os = "windows"),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(restore_bounds("login", None, size(px(720.), px(480.)), minimum, cx))),
        window_min_size: Some(minimum),
        ..Default::default()
    }
}

fn should_shutdown(closing_main_window: bool, open_windows: usize) -> bool {
    closing_main_window || open_windows == 0
}

fn shutdown(cx: &mut App) {
    let lifecycle = cx.global_mut::<AppLifecycle>();
    if lifecycle.quitting {
        return;
    }
    lifecycle.quitting = true;
    let session = Arc::clone(&lifecycle.session);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let viewport = lifecycle.viewport.take();
    for window in cx.windows() {
        if window.update(cx, |_, window, _| window.remove_window()).is_err() {
            tracing::trace!("Window was already unavailable during shutdown");
        }
    }
    cx.spawn(async move |cx| {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let worker = cx.background_executor().spawn(async move {
            if let Some(viewport) = viewport {
                viewport.shutdown();
            }
        });
        disconnect_session(session).await;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        worker.await;
        cx.update(|app| app.quit());
    })
    .detach();
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn start_scene(generation: u64, cx: &mut App) -> bool {
    use crate::gui::viewport::Viewport;
    use futures::StreamExt;

    if !cx.has_global::<AppLifecycle>()
        || matches!(cx.global::<AppLifecycle>().main, MainWindow::Starting(g) | MainWindow::Scene(g) if g >= generation)
    {
        return false;
    }
    if cx.global::<AppLifecycle>().viewport.is_none() {
        match Viewport::start() {
            Ok((viewport, mut events)) => {
                cx.global_mut::<AppLifecycle>().viewport = Some(viewport);
                cx.spawn(async move |cx| {
                    while let Some(event) = events.next().await {
                        cx.update(|cx| viewport_event(event, cx));
                    }
                    cx.update(|cx| {
                        let main = cx.global::<AppLifecycle>().main;
                        if let MainWindow::Starting(generation) | MainWindow::Scene(generation) = main {
                            scene_failed(generation, "The scene window stopped unexpectedly.".into(), cx);
                        }
                    });
                })
                .detach();
            }
            Err(error) => {
                cx.global_mut::<AppLifecycle>().main = MainWindow::Starting(generation);
                cx.defer(move |cx| scene_failed(generation, error.to_string(), cx));
                return false;
            }
        }
    }
    let lifecycle = cx.global_mut::<AppLifecycle>();
    lifecycle.main = MainWindow::Starting(generation);
    if !lifecycle.viewport.as_ref().is_some_and(|v| v.show(generation)) {
        cx.defer(move |cx| scene_failed(generation, "The scene window worker is unavailable.".into(), cx));
        return false;
    }
    true
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn connect_world(
    generation: u64,
    sources: crate::scene_feed::SceneSources,
    agent_id: Uuid,
    camera: watch::Sender<Option<Camera>>,
    cx: &mut App,
) {
    let lifecycle = cx.global::<AppLifecycle>();
    if !lifecycle.quitting
        && matches!(lifecycle.main, MainWindow::Starting(g) | MainWindow::Scene(g) if g == generation)
        && let Some(viewport) = &lifecycle.viewport
    {
        viewport.connect_world(generation, sources, agent_id, camera, service_runtime(cx));
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn end_scene(generation: u64, cx: &mut App) {
    if !cx.has_global::<AppLifecycle>() {
        return;
    }
    let lifecycle = cx.global_mut::<AppLifecycle>();
    if !matches!(lifecycle.main, MainWindow::Starting(g) | MainWindow::Scene(g) if g == generation) {
        return;
    }
    lifecycle.main = MainWindow::Login;
    if let Some(viewport) = &lifecycle.viewport {
        viewport.clear(generation);
    }
    cx.defer(show_login);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn viewport_event(event: crate::gui::viewport::ViewportEvent, cx: &mut App) {
    use crate::gui::viewport::ViewportEvent;
    let main = cx.global::<AppLifecycle>().main;
    if matches!(event, ViewportEvent::Chat(_))
        && let Some(viewport) = &cx.global::<AppLifecycle>().viewport
    {
        viewport.chat_received();
    }
    match event {
        ViewportEvent::Ready(generation) if main == MainWindow::Starting(generation) => {
            cx.global_mut::<AppLifecycle>().main = MainWindow::Scene(generation);
            show_chat(false, cx);
            if let Some(window) = cx.global_mut::<AppLifecycle>().login_window.take()
                && window.update(cx, |_, window, _| window.remove_window()).is_err()
            {
                tracing::trace!("Login window was already unavailable after scene startup");
            }
        }
        ViewportEvent::Chat(generation) if main == MainWindow::Scene(generation) => show_chat(true, cx),
        ViewportEvent::Closed(generation) if main == MainWindow::Scene(generation) || main == MainWindow::Starting(generation) => {
            shutdown(cx)
        }
        ViewportEvent::Failed(generation, error) => scene_failed(generation, error, cx),
        _ => {}
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn scene_failed(generation: u64, error: String, cx: &mut App) {
    let lifecycle = cx.global::<AppLifecycle>();
    if lifecycle.quitting || !matches!(lifecycle.main, MainWindow::Starting(g) | MainWindow::Scene(g) if g == generation) {
        return;
    }
    let controller = lifecycle.controller.clone();
    if let Some(controller) = controller {
        controller.update(cx, |login, cx| login.scene_failed(error, cx));
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn show_login(cx: &mut App) {
    let lifecycle = cx.global::<AppLifecycle>();
    if lifecycle.quitting {
        return;
    }
    if let Some(handle) = lifecycle.login_window {
        if handle.update(cx, |_, window, _| window.activate_window()).is_err() {
            tracing::trace!("Login window was already unavailable during activation");
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
    if let Some(chat) = cx.global_mut::<AppLifecycle>().chat_window.take()
        && chat.update(cx, |_, window, _| window.remove_window()).is_err()
    {
        tracing::trace!("Chat window was already unavailable during removal");
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn show_chat(focus: bool, cx: &mut App) {
    use crate::gui::chat::ChatWindow;
    let lifecycle = cx.global::<AppLifecycle>();
    if lifecycle.quitting {
        return;
    }
    if let Some(window) = lifecycle.chat_window {
        if focus && window.update(cx, |_, window, _| window.activate_window()).is_err() {
            tracing::trace!("Chat window was already unavailable during activation");
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
            appears_transparent: cfg!(target_os = "windows"),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(restore_bounds("chat", None, size(px(640.), px(520.)), minimum, cx))),
        window_min_size: Some(minimum),
        focus,
        ..Default::default()
    };
    match cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| ChatWindow::new(chat, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(window) => cx.global_mut::<AppLifecycle>().chat_window = Some(window.into()),
        Err(error) => tracing::error!(%error, "could not open nearby chat window"),
    }
}

async fn disconnect_session(session_slot: Arc<Mutex<Option<network::SessionHandle>>>) {
    let client = session_slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(client) = client else {
        return;
    };

    client.disconnect().await;
    session_slot.lock().unwrap_or_else(PoisonError::into_inner).take();
}

fn set_dark_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);

    let theme = Theme::global_mut(cx);
    theme.background = rgb_to_hsla(rgb(0x171717));
    theme.foreground = rgb_to_hsla(rgb(0xe6e6e6));
    theme.muted = rgb_to_hsla(rgb(0x202020));
    theme.muted_foreground = rgb_to_hsla(rgb(0x9b9b9b));
    theme.border = rgb_to_hsla(rgb(0x363636));
    theme.group_box = rgb_to_hsla(rgb(0x1b1b1b));
    theme.group_box_foreground = theme.foreground;
    theme.sidebar = rgb_to_hsla(rgb(0x191919));
    theme.sidebar_foreground = theme.foreground;
    theme.sidebar_border = theme.border;
    theme.input = rgb_to_hsla(rgb(0x222222));
    theme.button = rgb_to_hsla(rgb(0x252525));
    theme.button_foreground = theme.foreground;
    theme.button_hover = rgb_to_hsla(rgb(0x303030));
    theme.button_active = rgb_to_hsla(rgb(0x202020));
    theme.ring = rgb_to_hsla(rgb(0x898989));
    theme.caret = rgb_to_hsla(rgb(0xf2f2f2));
    theme.selection = rgb_to_hsla(rgb(0x3a3a3a));
    theme.accent = rgb_to_hsla(rgb(0x292929));
    theme.accent_foreground = rgb_to_hsla(rgb(0xe6e6e6));

    theme.primary = rgb_to_hsla(rgb(0x3a3a3a));
    theme.primary_foreground = rgb_to_hsla(rgb(0xf0f0f0));
    theme.primary_hover = rgb_to_hsla(rgb(0x484848));
    theme.primary_active = rgb_to_hsla(rgb(0x303030));
    theme.secondary = rgb_to_hsla(rgb(0x252525));
    theme.secondary_foreground = theme.foreground;
    theme.secondary_hover = rgb_to_hsla(rgb(0x303030));
    theme.secondary_active = rgb_to_hsla(rgb(0x202020));

    theme.link = rgb_to_hsla(rgb(0xc6c6c6));
    theme.link_hover = rgb_to_hsla(rgb(0xe0e0e0));
    theme.link_active = rgb_to_hsla(rgb(0xababab));
    theme.title_bar = theme.background;
    theme.title_bar_border = theme.background;

    theme.danger = rgb_to_hsla(rgb(0xe81120));
    theme.danger_hover = rgb_to_hsla(rgb(0xe81120));
    theme.danger_active = rgb_to_hsla(rgb(0xc50f1f));
    theme.danger_foreground = rgb_to_hsla(rgb(0xffffff));
    theme.tokens = theme.colors.into();
    theme.tokens.table = theme.group_box.into();
    theme.tokens.table_head = theme.group_box.into();
    theme.table_active = rgb_to_hsla(rgb(0x666666)).opacity(0.35);
    theme.table_active_border = theme.table_active.opacity(0.0);
    theme.tokens.table_active = theme.table_active.into();
    Theme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::should_shutdown;
    use gpui_component_assets::Assets;

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[gpui::test]
    fn handoff_preserves_window_lifetimes(cx: &mut gpui::TestAppContext) {
        use super::{AppLifecycle, MainWindow, install_lifecycle, start_scene, viewport_event};
        use crate::gui::{login::LoginScreen, viewport::ViewportEvent};
        use std::sync::{Arc, Mutex};

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
            lifecycle.main = MainWindow::Starting(2);
            window.window_handle().window_id()
        });
        cx.update(|cx| {
            viewport_event(ViewportEvent::Ready(1), cx);
            assert_eq!(cx.global::<AppLifecycle>().main, MainWindow::Starting(2));
            assert!(!start_scene(2, cx), "repeated readiness must not restart a pending scene");
            viewport_event(ViewportEvent::Ready(2), cx);
            let lifecycle = cx.global::<AppLifecycle>();
            assert_eq!(lifecycle.main, MainWindow::Scene(2));
            assert!(lifecycle.login_window.is_none());
            assert_ne!(lifecycle.chat_window.unwrap().window_id(), login_id, "chat must have its own native window");
            assert!(!start_scene(2, cx), "repeated readiness must not restart the active scene");
            assert!(!start_scene(1, cx), "older readiness must not replace the active scene");
            assert_eq!(cx.global::<AppLifecycle>().main, MainWindow::Scene(2));
            viewport_event(ViewportEvent::Closed(1), cx);
            viewport_event(ViewportEvent::Failed(1, "stale failure".into()), cx);
            assert!(!cx.global::<AppLifecycle>().quitting);
            let chat = cx.global::<AppLifecycle>().chat_window.unwrap();
            chat.update(cx, |_, window, _| window.remove_window()).unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let lifecycle = cx.global::<AppLifecycle>();
            assert!(lifecycle.login_window.is_none());
            assert!(lifecycle.chat_window.is_none());
            assert_eq!(lifecycle.main, MainWindow::Scene(2));
            assert!(!lifecycle.quitting, "closing the last GPUI window must leave the scene and session alive");
            viewport_event(ViewportEvent::Chat(2), cx);
            assert!(cx.global::<AppLifecycle>().chat_window.is_some(), "chat must reopen without replacing the session controller");
            assert_eq!(cx.global::<AppLifecycle>().controller.as_ref().unwrap().entity_id(), controller.entity_id());
            viewport_event(ViewportEvent::Closed(2), cx);
            assert!(cx.global::<AppLifecycle>().quitting, "closing the current scene owns shutdown");
        });
        cx.run_until_parked();
    }

    #[test]
    fn tool_close_keeps_app_open() {
        assert!(!should_shutdown(false, 1));
    }

    #[test]
    fn sortable_icons_are_embedded() {
        for path in ["icons/check.svg", "icons/chevrons-up-down.svg", "icons/sort-ascending.svg", "icons/sort-descending.svg"] {
            assert!(Assets.load(path).expect("icon assets should load").is_some(), "missing {path}");
        }
    }

    #[test]
    fn main_close_shuts_down() {
        assert!(should_shutdown(true, 1));
    }

    #[test]
    fn last_window_close_shuts_down() {
        assert!(should_shutdown(false, 0));
    }
}
