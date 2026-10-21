use gpui::AssetSource;

use super::{Assets, should_shutdown};

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[gpui::test]
fn handoff_preserves_window_lifetimes(cx: &mut gpui::TestAppContext) {
    use super::{AppLifecycle, MainWindow, install_lifecycle, start_scene, viewport_event};
    use crate::{login::LoginScreen, viewport::ViewportEvent};
    use std::sync::{Arc, Mutex, atomic::AtomicBool};

    let session = Arc::new(Mutex::new(None));
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_quit_mode(gpui::QuitMode::Explicit);
        install_lifecycle(Arc::clone(&session), cx);
    });
    let (controller, visual) = cx.add_window_view(|window, cx| LoginScreen::new(window, cx, Arc::new(AtomicBool::new(false)), session));
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
