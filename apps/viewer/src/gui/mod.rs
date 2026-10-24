mod components;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod scene;
mod views;
mod window_geometry;

use std::sync::{Arc, Mutex, PoisonError};

use gpui::{
    AnyWindowHandle, App, AppContext, ColorExt, Entity, Global, QuitMode, Subscription, TitlebarOptions, WindowBounds, WindowOptions, px,
    rgb, rgb_to_hsla, size,
};
use gpui_component::{Root, Theme, ThemeMode};
use tokio::runtime::Handle;

use crate::{gui::views::login::LoginScreen, gui::window_geometry::restore_bounds};

struct AppLifecycle {
    _window_closed: Subscription,
    controller: Option<Entity<LoginScreen>>,
    login_window: Option<AnyWindowHandle>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    scene: scene::State,
    quitting: bool,
    session: Arc<Mutex<Option<network::SessionHandle>>>,
}

impl Global for AppLifecycle {}

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
        if lifecycle.scene.window_closed(closed_window_id) {
            return;
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
        scene: scene::State::new(),
        quitting: false,
        session,
    });
}

fn login_options(cx: &App) -> WindowOptions {
    let minimum = size(px(460.), px(420.));
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Radia".into()),
            appears_transparent: components::window_chrome::appears_transparent(),
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
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let background_executor = cx.background_executor().clone();
    let lifecycle = cx.global_mut::<AppLifecycle>();
    if lifecycle.quitting {
        return;
    }
    lifecycle.quitting = true;
    let session = Arc::clone(&lifecycle.session);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let scene_shutdown = lifecycle.scene.shutdown(background_executor);
    for window in cx.windows() {
        if window.update(cx, |_, window, _| window.remove_window()).is_err() {
            tracing::trace!("window was already unavailable during shutdown");
        }
    }
    cx.spawn(async move |cx| {
        disconnect_session(session).await;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        scene_shutdown.await;
        cx.update(|app| app.quit());
    })
    .detach();
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
    use super::should_shutdown;

    #[test]
    fn tool_close_keeps_app_open() {
        assert!(!should_shutdown(false, 1));
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
