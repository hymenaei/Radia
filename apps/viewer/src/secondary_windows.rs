use std::collections::HashSet;

#[cfg(target_os = "windows")]
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

#[cfg(target_os = "linux")]
use std::ffi::c_void;

use gpui::{App, Global, Window, WindowId};

#[cfg(target_os = "windows")]
type NativeSecondaryWindows = Arc<Mutex<NativeSecondaryWindowState>>;

#[cfg(target_os = "windows")]
#[derive(Default)]
struct NativeSecondaryWindowState {
    windows: HashMap<u64, isize>,
    minimized_with_main: HashSet<isize>,
    main_minimized: bool,
}

#[derive(Clone, Default)]
pub(crate) struct SecondaryWindowRegistry {
    windows: HashSet<WindowId>,
    #[cfg(target_os = "windows")]
    native_windows: NativeSecondaryWindows,
}

impl SecondaryWindowRegistry {
    fn register(&mut self, window_id: WindowId) -> bool {
        self.windows.insert(window_id)
    }

    fn unregister(&mut self, window_id: WindowId) -> bool {
        self.windows.remove(&window_id)
    }

    fn contains(&self, window_id: &WindowId) -> bool {
        self.windows.contains(window_id)
    }
}

impl Global for SecondaryWindowRegistry {}

pub(crate) fn initialize(cx: &mut App) {
    cx.set_global(SecondaryWindowRegistry::default());
}

pub(crate) fn register(window: &Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();

    #[cfg(target_os = "windows")]
    let native_handle = native_window_handle(window);
    let registry = cx.global_mut::<SecondaryWindowRegistry>();
    registry.register(window_id);

    #[cfg(target_os = "windows")]
    if let Some(raw_window_handle::RawWindowHandle::Win32(handle)) = native_handle {
        registry.native_windows.lock().unwrap_or_else(PoisonError::into_inner).windows.insert(window_id.as_u64(), handle.hwnd.get());
    }
}

pub(crate) fn unregister(window_id: WindowId, cx: &mut App) {
    if cx.has_global::<SecondaryWindowRegistry>() {
        let registry = cx.global_mut::<SecondaryWindowRegistry>();
        registry.unregister(window_id);

        #[cfg(target_os = "windows")]
        {
            let mut state = registry.native_windows.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(hwnd) = state.windows.remove(&window_id.as_u64()) {
                state.minimized_with_main.remove(&hwnd);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn install_main_window_z_order_guard(window: &Window, cx: &App) -> bool {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;

    let Some(RawWindowHandle::Win32(native)) = native_window_handle(window) else {
        return false;
    };
    let Some(registry) = cx.try_global::<SecondaryWindowRegistry>() else {
        return false;
    };

    install_windows_z_order_guard(HWND(native.hwnd.get() as *mut _), Arc::clone(&registry.native_windows))
}

#[cfg(target_os = "windows")]
fn install_windows_z_order_guard(hwnd: windows::Win32::Foundation::HWND, windows: NativeSecondaryWindows) -> bool {
    use windows::Win32::UI::Shell::{SUBCLASSPROC, SetWindowSubclass};

    let reference = Box::into_raw(Box::new(windows)) as usize;
    let subclass: SUBCLASSPROC = Some(main_window_subclass);
    if unsafe { SetWindowSubclass(hwnd, subclass, 1, reference).as_bool() } {
        true
    } else {
        // SAFETY: SetWindowSubclass rejected the subclass, so it will never use this pointer.
        unsafe { drop(Box::from_raw(reference as *mut NativeSecondaryWindows)) };
        false
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn main_window_subclass(
    hwnd: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    subclass_id: usize,
    reference: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass},
        WindowsAndMessaging::{SWP_NOZORDER, WINDOWPOS, WM_NCDESTROY, WM_SIZE, WM_WINDOWPOSCHANGING},
    };

    if message == WM_NCDESTROY {
        let subclass: windows::Win32::UI::Shell::SUBCLASSPROC = Some(main_window_subclass);
        // SAFETY: WM_NCDESTROY is final for this window and owns the boxed reference installed above.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, subclass, subclass_id);
            drop(Box::from_raw(reference as *mut NativeSecondaryWindows));
        }
    }

    if message == WM_WINDOWPOSCHANGING {
        let position = lparam.0 as *mut WINDOWPOS;
        if !position.is_null() {
            // SAFETY: Windows supplies a valid WINDOWPOS pointer for WM_WINDOWPOSCHANGING.
            let position = unsafe { &mut *position };
            if position.flags.0 & SWP_NOZORDER.0 == 0 {
                // SAFETY: SetWindowSubclass stores this pointer until WM_NCDESTROY, where it is freed.
                let native_windows = unsafe { &*(reference as *const NativeSecondaryWindows) };
                if let Some(safe_insert_after) = safe_main_insert_after(hwnd, position.hwndInsertAfter, native_windows) {
                    position.hwndInsertAfter = safe_insert_after;
                }
            }
        }
    }

    if message == WM_SIZE {
        // SAFETY: SetWindowSubclass stores this pointer until WM_NCDESTROY.
        let state = unsafe { &*(reference as *const NativeSecondaryWindows) };
        let action = {
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            main_window_size_action(&mut state, wparam.0 as u32, |raw| {
                let secondary = windows::Win32::Foundation::HWND(raw as *mut _);
                unsafe {
                    windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(secondary)).as_bool()
                        && windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(secondary).as_bool()
                        && !windows::Win32::UI::WindowsAndMessaging::IsIconic(secondary).as_bool()
                }
            })
        };
        apply_main_window_size_action(action);
    }

    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(target_os = "windows")]
#[derive(Debug, PartialEq, Eq)]
enum MainWindowSizeAction {
    Minimize(Vec<isize>),
    Restore(Vec<isize>),
}

#[cfg(target_os = "windows")]
fn main_window_size_action(
    state: &mut NativeSecondaryWindowState,
    size_type: u32,
    mut is_visible_and_restored: impl FnMut(isize) -> bool,
) -> Option<MainWindowSizeAction> {
    use windows::Win32::UI::WindowsAndMessaging::{SIZE_MAXIMIZED, SIZE_MINIMIZED, SIZE_RESTORED};

    if size_type == SIZE_MINIMIZED {
        if state.main_minimized {
            return None;
        }

        let windows: Vec<_> = state.windows.values().copied().filter(|window| is_visible_and_restored(*window)).collect();
        state.main_minimized = true;
        state.minimized_with_main.extend(windows.iter().copied());
        Some(MainWindowSizeAction::Minimize(windows))
    } else if size_type == SIZE_RESTORED || size_type == SIZE_MAXIMIZED {
        if !state.main_minimized {
            return None;
        }
        state.main_minimized = false;
        Some(MainWindowSizeAction::Restore(state.minimized_with_main.drain().collect()))
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn apply_main_window_size_action(action: Option<MainWindowSizeAction>) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{IsIconic, IsWindow, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE, ShowWindowAsync},
    };

    let Some(action) = action else { return };
    let (windows, command, should_be_iconic) = match action {
        MainWindowSizeAction::Minimize(windows) => (windows, SW_SHOWMINNOACTIVE, false),
        MainWindowSizeAction::Restore(windows) => (windows, SW_SHOWNOACTIVATE, true),
    };

    for raw in windows {
        let hwnd = HWND(raw as *mut _);
        let is_window = unsafe { IsWindow(Some(hwnd)).as_bool() };
        let is_iconic = unsafe { IsIconic(hwnd).as_bool() };
        if is_window && is_iconic == should_be_iconic {
            let _ = unsafe { ShowWindowAsync(hwnd, command) };
        }
    }
}

#[cfg(target_os = "windows")]
fn safe_main_insert_after(
    main: windows::Win32::Foundation::HWND,
    requested_insert_after: windows::Win32::Foundation::HWND,
    native_windows: &NativeSecondaryWindows,
) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{GW_HWNDNEXT, GetTopWindow, GetWindow, IsIconic, IsWindowVisible};

    let native_windows = native_windows.lock().unwrap_or_else(PoisonError::into_inner);
    let mut window = unsafe { GetTopWindow(None).ok()? };
    let mut last_secondary = None;
    let mut requested_position = None;
    let mut last_secondary_position = None;
    let mut position = 0;
    while !window.0.is_null() && window != main {
        if window == requested_insert_after {
            requested_position = Some(position);
        }
        let registered = native_windows.windows.values().any(|handle| *handle == window.0 as isize);
        if registered && unsafe { IsWindowVisible(window).as_bool() } && !unsafe { IsIconic(window).as_bool() } {
            last_secondary = Some(window);
            last_secondary_position = Some(position);
        }
        let Ok(next) = (unsafe { GetWindow(window, GW_HWNDNEXT) }) else {
            break;
        };
        window = next;
        position += 1;
    }

    if requested_insert_after == windows::Win32::UI::WindowsAndMessaging::HWND_TOP
        || requested_position.zip(last_secondary_position).is_some_and(|(requested, secondary)| requested < secondary)
    {
        last_secondary
    } else {
        None
    }
}

pub(crate) fn raise_above_main_if_active(main: &Window, cx: &mut App) {
    if !main.is_window_active() {
        return;
    }

    let Some(registry) = cx.try_global::<SecondaryWindowRegistry>().cloned() else {
        return;
    };

    for handle in cx.windows() {
        if registry.contains(&handle.window_id()) {
            let _ = handle.update(cx, |_, window, _| raise_without_activating(window));
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn remove_from_taskbar(window: &Window) -> bool {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;

    let Some(RawWindowHandle::Win32(native)) = native_window_handle(window) else {
        return false;
    };
    remove_taskbar_button(HWND(native.hwnd.get() as *mut _))
}

#[cfg(target_os = "windows")]
fn remove_taskbar_button(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize},
        UI::Shell::ITaskbarList,
    };

    let initialized_here = match unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) } {
        result if result.is_ok() => true,
        result if result == RPC_E_CHANGED_MODE => false,
        _ => return false,
    };

    let result = unsafe {
        const CLSID_TASKBAR_LIST: windows::core::GUID = windows::core::GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);

        CoCreateInstance::<_, ITaskbarList>(&CLSID_TASKBAR_LIST, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER)
            .and_then(|taskbar| taskbar.HrInit().and_then(|()| taskbar.DeleteTab(hwnd)))
    };

    if initialized_here {
        unsafe { CoUninitialize() };
    }

    result.is_ok()
}

fn native_window_handle(window: &Window) -> Option<raw_window_handle::RawWindowHandle> {
    raw_window_handle::HasWindowHandle::window_handle(window).ok().map(|handle| handle.as_raw())
}

fn raise_without_activating(window: &Window) {
    #[cfg(target_os = "windows")]
    raise_windows_without_activating(window);

    #[cfg(target_os = "macos")]
    raise_macos_without_activating(window);

    #[cfg(target_os = "linux")]
    raise_x11_without_activating(window);

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    let _ = window;
}

#[cfg(target_os = "windows")]
fn raise_windows_without_activating(window: &Window) {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::HWND;

    let Some(RawWindowHandle::Win32(native)) = native_window_handle(window) else {
        return;
    };
    raise_windows_hwnd(HWND(native.hwnd.get() as *mut _));
}

#[cfg(target_os = "windows")]
fn raise_windows_hwnd(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, IsIconic, IsWindowVisible, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SetWindowPos,
    };

    if !unsafe { IsWindowVisible(hwnd).as_bool() } || unsafe { IsIconic(hwnd).as_bool() } {
        return false;
    }

    unsafe { SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER) }.is_ok()
}

#[cfg(target_os = "macos")]
fn raise_macos_without_activating(window: &Window) {
    use objc::{
        msg_send,
        runtime::{NO, Object, YES},
        sel, sel_impl,
    };
    use raw_window_handle::RawWindowHandle;
    use std::ptr::null_mut;

    let Some(RawWindowHandle::AppKit(native)) = native_window_handle(window) else {
        return;
    };

    unsafe {
        let view = native.ns_view.as_ptr() as *mut Object;
        let native_window: *mut Object = msg_send![view, window];
        if native_window.is_null() {
            return;
        }
        let visible: objc::runtime::BOOL = msg_send![native_window, isVisible];
        let miniaturized: objc::runtime::BOOL = msg_send![native_window, isMiniaturized];
        if visible == YES && miniaturized == NO {
            let _: () = msg_send![native_window, orderFront: null_mut::<Object>()];
        }
    }
}

#[cfg(target_os = "linux")]
fn raise_x11_without_activating(window: &Window) {
    use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

    let Some(RawWindowHandle::Xcb(native)) = native_window_handle(window) else {
        return;
    };
    let Ok(display) = raw_window_handle::HasDisplayHandle::display_handle(window) else {
        return;
    };
    let RawDisplayHandle::Xcb(display) = display.as_raw() else {
        return;
    };
    let Some(connection) = display.connection else {
        return;
    };

    unsafe {
        let stack_mode = [XCB_STACK_MODE_ABOVE];
        xcb_configure_window(connection.as_ptr(), native.window.get(), XCB_CONFIG_WINDOW_STACK_MODE, stack_mode.as_ptr().cast());
        xcb_flush(connection.as_ptr());
    }
}

#[cfg(target_os = "linux")]
const XCB_CONFIG_WINDOW_STACK_MODE: u16 = 1 << 6;

#[cfg(target_os = "linux")]
const XCB_STACK_MODE_ABOVE: u32 = 0;

#[cfg(target_os = "linux")]
#[repr(C)]
struct XcbVoidCookie {
    sequence: u32,
}

#[cfg(target_os = "linux")]
#[link(name = "xcb")]
unsafe extern "C" {
    fn xcb_configure_window(connection: *mut c_void, window: u32, value_mask: u16, value_list: *const c_void) -> XcbVoidCookie;
    fn xcb_flush(connection: *mut std::ffi::c_void) -> i32;
}

#[cfg(test)]
mod tests {
    use super::SecondaryWindowRegistry;
    use gpui::WindowId;

    #[test]
    fn registration_is_idempotent_and_removable() {
        let window = WindowId::from(1);
        let mut registry = SecondaryWindowRegistry::default();

        assert!(registry.register(window));
        assert!(!registry.register(window));
        assert!(registry.contains(&window));

        assert!(registry.unregister(window));
        assert!(!registry.contains(&window));
        assert!(!registry.unregister(window));
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        thread::sleep,
        time::Duration,
    };

    use super::{NativeSecondaryWindows, install_windows_z_order_guard, raise_windows_hwnd};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GW_HWNDNEXT, GetForegroundWindow, GetTopWindow, GetWindow, HWND_TOP, IsIconic, IsWindowVisible,
            MSG, PM_REMOVE, PeekMessageW, SIZE_MAXIMIZED, SIZE_MINIMIZED, SIZE_RESTORED, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SetWindowPos, ShowWindow, WS_OVERLAPPEDWINDOW,
        },
    };

    use super::{MainWindowSizeAction, NativeSecondaryWindowState, main_window_size_action};

    struct TestWindow(HWND);

    impl TestWindow {
        fn create() -> Self {
            let hwnd = unsafe {
                CreateWindowExW(
                    Default::default(),
                    windows::core::w!("STATIC"),
                    windows::core::w!(""),
                    WS_OVERLAPPEDWINDOW,
                    -32000,
                    -32000,
                    1,
                    1,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("create native test window");
            Self(hwnd)
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn raise_skips_hidden_or_minimized_and_preserves_focus() {
        let foreground = unsafe { GetForegroundWindow() };

        let hidden = TestWindow::create();
        assert!(!raise_windows_hwnd(hidden.0));
        assert!(!unsafe { IsWindowVisible(hidden.0).as_bool() });

        let minimized = TestWindow::create();
        let _ = unsafe { ShowWindow(minimized.0, SW_SHOWMINNOACTIVE) };
        assert!(unsafe { IsIconic(minimized.0).as_bool() });
        assert!(!raise_windows_hwnd(minimized.0));
        assert!(unsafe { IsIconic(minimized.0).as_bool() });

        let visible = TestWindow::create();
        let _ = unsafe { ShowWindow(visible.0, SW_SHOWNOACTIVATE) };
        assert!(unsafe { IsWindowVisible(visible.0).as_bool() });
        assert!(raise_windows_hwnd(visible.0));
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    }

    #[test]
    fn main_native_raise_preserves_secondary_z_order() {
        let main = TestWindow::create();
        let secondary = TestWindow::create();
        let _ = unsafe { ShowWindow(main.0, SW_SHOWNOACTIVATE) };
        let _ = unsafe { ShowWindow(secondary.0, SW_SHOWNOACTIVATE) };
        assert!(raise_windows_hwnd(secondary.0));
        assert!(is_above(secondary.0, main.0));
        let native_windows: NativeSecondaryWindows = Arc::new(Mutex::new(NativeSecondaryWindowState {
            windows: HashMap::from([(1, secondary.0.0 as isize)]),
            ..Default::default()
        }));
        assert!(install_windows_z_order_guard(main.0, native_windows));

        unsafe { SetWindowPos(main.0, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER) }
            .expect("raise main test window");

        assert!(is_above(secondary.0, main.0));
    }

    fn is_above(first: HWND, second: HWND) -> bool {
        let Ok(mut window) = (unsafe { GetTopWindow(None) }) else {
            return false;
        };
        while !window.0.is_null() {
            if window == first {
                return true;
            }
            if window == second {
                return false;
            }
            let Ok(next) = (unsafe { GetWindow(window, GW_HWNDNEXT) }) else {
                break;
            };
            window = next;
        }
        false
    }

    #[test]
    fn main_minimize_handles_secondaries() {
        let secondary = TestWindow::create();
        let _ = unsafe { ShowWindow(secondary.0, SW_SHOWNOACTIVATE) };
        let mut state = NativeSecondaryWindowState { windows: HashMap::from([(1, secondary.0.0 as isize)]), ..Default::default() };

        assert_eq!(
            main_window_size_action(&mut state, SIZE_MINIMIZED, |_| true),
            Some(MainWindowSizeAction::Minimize(vec![secondary.0.0 as isize]))
        );
        assert!(state.main_minimized);
        assert_eq!(main_window_size_action(&mut state, SIZE_MINIMIZED, |_| true), None);
        assert!(!unsafe { IsIconic(secondary.0).as_bool() });
        assert_eq!(state.minimized_with_main, [secondary.0.0 as isize].into());

        assert_eq!(
            main_window_size_action(&mut state, SIZE_RESTORED, |_| true),
            Some(MainWindowSizeAction::Restore(vec![secondary.0.0 as isize]))
        );
    }

    #[test]
    fn taskbar_minimize_restores_windows_in_prior_state() {
        let visible = TestWindow::create();
        let previously_minimized = TestWindow::create();
        let hidden = TestWindow::create();
        let _ = unsafe { ShowWindow(visible.0, SW_SHOWNOACTIVATE) };
        let _ = unsafe { ShowWindow(previously_minimized.0, SW_SHOWMINNOACTIVE) };
        let mut state = NativeSecondaryWindowState {
            windows: HashMap::from([(1, visible.0.0 as isize), (2, previously_minimized.0.0 as isize), (3, hidden.0.0 as isize)]),
            ..Default::default()
        };

        let minimize = main_window_size_action(&mut state, SIZE_MINIMIZED, |raw| {
            let hwnd = HWND(raw as *mut _);
            unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() }
        });
        assert_eq!(minimize, Some(MainWindowSizeAction::Minimize(vec![visible.0.0 as isize])));
        assert!(state.main_minimized);
        assert_eq!(state.minimized_with_main, [visible.0.0 as isize].into());
        let foreground = unsafe { GetForegroundWindow() };
        super::apply_main_window_size_action(minimize);
        assert!(wait_until(|| unsafe { IsIconic(visible.0).as_bool() }));
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);

        let restore = main_window_size_action(&mut state, SIZE_RESTORED, |_| false);
        assert_eq!(restore, Some(MainWindowSizeAction::Restore(vec![visible.0.0 as isize])));
        assert!(!state.main_minimized);
        assert!(state.minimized_with_main.is_empty());
        super::apply_main_window_size_action(restore);
        assert!(wait_until(|| unsafe { !IsIconic(visible.0).as_bool() }));
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        assert!(unsafe { IsWindowVisible(visible.0).as_bool() });
        assert!(unsafe { IsIconic(previously_minimized.0).as_bool() });
        assert!(!unsafe { IsWindowVisible(hidden.0).as_bool() });
    }

    #[test]
    fn repeated_minimize_preserves_restore_set() {
        let mut state = NativeSecondaryWindowState { windows: HashMap::from([(1, 100_isize)]), main_minimized: true, ..Default::default() };

        assert_eq!(main_window_size_action(&mut state, SIZE_MINIMIZED, |_| true), None);
        assert!(state.minimized_with_main.is_empty());
        assert_eq!(main_window_size_action(&mut state, SIZE_MAXIMIZED, |_| true), Some(MainWindowSizeAction::Restore(Vec::new())));
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&message);
                    windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&message);
                }
            }
            if predicate() {
                return true;
            }
            sleep(Duration::from_millis(5));
        }
        predicate()
    }
}
