use std::sync::{Arc, Mutex, atomic::AtomicBool};

use crate::{diagnostics, login::LoginScreen};
use gpui::{
    App, AppContext, Bounds, Global, QuitMode, Subscription, TitlebarOptions, WindowBounds, WindowOptions, px, rgb, rgb_to_hsla, size,
};
use gpui_component::{Root, Theme, ThemeMode};

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

    let capture_enabled = match std::env::args_os().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [argument] if argument == "--capture" => true,
        _ => {
            eprintln!("Usage: radia [--capture]");
            std::process::exit(2);
        }
    };

    let capture_on_login = Arc::new(AtomicBool::new(capture_enabled));
    let shutdown_session: Arc<Mutex<Option<network::SessionHandle>>> = Arc::new(Mutex::new(None));

    gpui_platform::application().run(move |cx: &mut App| {
        cx.set_quit_mode(QuitMode::Explicit);

        gpui_component::init(cx);
        set_dark_theme(cx);

        let session_slot = Arc::clone(&shutdown_session);
        let window_closed = cx.on_window_closed(move |cx, _window_id| {
            if !cx.windows().is_empty() {
                return;
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

        let bounds = Bounds::centered(None, size(px(720.), px(480.)), cx);
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Radia".into()),
                    appears_transparent: cfg!(target_os = "windows"),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
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
        cx.activate(true);
    });
}

async fn disconnect_session(session_slot: Arc<Mutex<Option<network::SessionHandle>>>) -> bool {
    let client = session_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let Some(client) = client else {
        return false;
    };

    let capture_active = client.capture_status().is_some_and(|status| status.active);
    client.disconnect().await;
    session_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    capture_active
}

fn set_dark_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);

    let theme = Theme::global_mut(cx);
    theme.background = rgb_to_hsla(rgb(0x202020));
    theme.foreground = rgb_to_hsla(rgb(0xe6e6e6));
    theme.muted = rgb_to_hsla(rgb(0x292929));
    theme.muted_foreground = rgb_to_hsla(rgb(0xa0a0a0));
    theme.border = rgb_to_hsla(rgb(0x414141));
    theme.input = rgb_to_hsla(rgb(0x4a4a4a));
    theme.ring = rgb_to_hsla(rgb(0xb3b3b3));
    theme.caret = rgb_to_hsla(rgb(0xf2f2f2));
    theme.selection = rgb_to_hsla(rgb(0x4a4a4a));

    theme.primary = rgb_to_hsla(rgb(0xd8d8d8));
    theme.primary_foreground = rgb_to_hsla(rgb(0x202020));
    theme.primary_hover = rgb_to_hsla(rgb(0xf0f0f0));
    theme.primary_active = rgb_to_hsla(rgb(0xc6c6c6));
    theme.secondary = rgb_to_hsla(rgb(0x2d2d2d));
    theme.secondary_foreground = rgb_to_hsla(rgb(0xe6e6e6));
    theme.secondary_hover = rgb_to_hsla(rgb(0x393939));
    theme.secondary_active = rgb_to_hsla(rgb(0x333333));

    theme.link = rgb_to_hsla(rgb(0xc9c9c9));
    theme.link_hover = rgb_to_hsla(rgb(0xffffff));
    theme.link_active = rgb_to_hsla(rgb(0xb0b0b0));
    theme.title_bar = rgb_to_hsla(rgb(0x202020));
    theme.title_bar_border = rgb_to_hsla(rgb(0x202020));

    theme.danger = rgb_to_hsla(rgb(0xe81120));
    theme.danger_hover = rgb_to_hsla(rgb(0xe81120));
    theme.danger_active = rgb_to_hsla(rgb(0xc50f1f));
    theme.danger_foreground = rgb_to_hsla(rgb(0xffffff));
}
