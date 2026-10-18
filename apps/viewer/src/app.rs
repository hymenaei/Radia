use std::{
    env::args_os,
    process::exit,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{diagnostics, login::LoginScreen, window_geometry::restore_bounds};
use gpui::{
    App, AppContext, ColorExt, Global, QuitMode, Subscription, TitlebarOptions, WindowBounds, WindowOptions, px, rgb, rgb_to_hsla, size,
};
use gpui_component::{Root, Theme, ThemeMode};
use gpui_component_assets::Assets;
use gpui_platform::application;

struct AppLifecycle {
    _window_closed: Subscription,
}

impl Global for AppLifecycle {}

/// Runs the viewer until its main window closes.
///
/// # Panics
///
/// Panics if the application cannot open its main window.
pub fn run() {
    diagnostics::init();

    let capture_enabled = match args_os().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--capture" => true,
        _ => {
            eprintln!("Usage: radia [--capture]");
            exit(2);
        }
    };

    let capture_on_login = Arc::new(AtomicBool::new(capture_enabled));
    let shutdown_session: Arc<Mutex<Option<network::SessionHandle>>> = Arc::new(Mutex::new(None));
    let main_window_id = Arc::new(Mutex::new(None));
    let shutdown_started = Arc::new(AtomicBool::new(false));

    application().with_assets(Assets).run(move |cx: &mut App| {
        cx.set_quit_mode(QuitMode::Explicit);

        gpui_component::init(cx);
        set_dark_theme(cx);

        let session_slot = Arc::clone(&shutdown_session);
        let main_window_id_on_close = Arc::clone(&main_window_id);
        let shutdown_started_on_close = Arc::clone(&shutdown_started);
        let window_closed = cx.on_window_closed(move |cx, closed_window_id| {
            let main_window_id = *main_window_id_on_close.lock().unwrap_or_else(PoisonError::into_inner);
            let closing_main_window = main_window_id == Some(closed_window_id.as_u64());
            let open_windows = cx.windows();
            if !should_shutdown(closing_main_window, open_windows.len()) {
                return;
            }
            if shutdown_started_on_close.swap(true, Ordering::Relaxed) {
                return;
            }

            if closing_main_window {
                for handle in open_windows {
                    if handle.window_id() != closed_window_id {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
            }

            let session_slot = Arc::clone(&session_slot);
            cx.spawn(async move |cx| {
                let interrupted_capture = disconnect_session(session_slot).await;
                if interrupted_capture {
                    println!("Capture interrupted and saved locally.");
                }
                cx.update(|app| app.quit());
            })
            .detach();
        });
        cx.set_global(AppLifecycle { _window_closed: window_closed });

        let minimum_size = size(px(460.), px(420.));
        let bounds = restore_bounds("main", None, size(px(720.), px(480.)), minimum_size, cx);
        let main_window = cx
            .open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        title: Some("Radia".into()),
                        appears_transparent: cfg!(target_os = "windows"),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(minimum_size),
                    ..Default::default()
                },
                move |window, cx| {
                    cx.new(|cx| {
                        let login = cx.new(|cx| LoginScreen::new(window, cx, capture_on_login, shutdown_session));
                        Root::new(login, window, cx)
                    })
                },
            )
            .expect("failed to open the main window");
        *main_window_id.lock().unwrap_or_else(PoisonError::into_inner) = Some(main_window.window_id().as_u64());
        cx.activate(true);
    });
}

fn should_shutdown(closing_main_window: bool, open_windows: usize) -> bool {
    closing_main_window || open_windows == 0
}

async fn disconnect_session(session_slot: Arc<Mutex<Option<network::SessionHandle>>>) -> bool {
    let client = session_slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(client) = client else {
        return false;
    };

    let capture_active = client.capture_status().is_some_and(|status| status.active);
    client.disconnect().await;
    session_slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    capture_active
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

    use super::{Assets, should_shutdown};

    #[test]
    fn closing_a_tool_window_keeps_the_main_app_open() {
        assert!(!should_shutdown(false, 1));
    }

    #[test]
    fn sortable_icons_are_embedded() {
        for path in ["icons/check.svg", "icons/chevrons-up-down.svg", "icons/sort-ascending.svg", "icons/sort-descending.svg"] {
            assert!(Assets.load(path).expect("icon assets should load").is_some(), "missing {path}");
        }
    }

    #[test]
    fn closing_the_main_window_shuts_down_with_tools_open() {
        assert!(should_shutdown(true, 1));
    }

    #[test]
    fn closing_the_last_window_shuts_down() {
        assert!(should_shutdown(false, 0));
    }
}
