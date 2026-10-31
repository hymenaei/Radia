use std::time::Instant;

use winit::{
    dpi::PhysicalPosition,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    keyboard::{Key, ModifiersState, NamedKey},
    window::CursorGrabMode,
};

use crate::{
    DEFAULT_DRAW_DISTANCE,
    agent_movement::KeyBinding,
    camera::{CameraMode, handle_keyboard_input, should_reset_sitting_orbit},
    gui::views::viewport::{
        InputCapture,
        sit_controls::{SitMenuEvent, sit_request_for_left_click},
    },
    scene_feed::SceneFeed,
};

use super::SceneWindow;

pub(super) const CAMERA_DRAG_THRESHOLD: f64 = 4.0;

#[derive(Default)]
pub(super) struct ViewportInput {
    drag: Option<DragState>,
    sit_click: Option<SitClick>,
}

impl ViewportInput {
    pub(super) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub(super) fn begin_sit_click(&mut self, position: PhysicalPosition<f64>, enabled: bool) {
        self.sit_click = enabled.then_some(SitClick { position, travel: [0.0; 2] });
    }

    fn cancel_sit_click(&mut self) {
        self.sit_click = None;
    }

    pub(super) fn pointer_motion(&mut self, (dx, dy): (f64, f64), dpi: f64) {
        let crossed_threshold = self.sit_click.as_mut().is_some_and(|click| {
            click.travel[0] += dx.abs() / dpi;
            click.travel[1] += dy.abs() / dpi;
            click.travel[0] >= CAMERA_DRAG_THRESHOLD || click.travel[1] >= CAMERA_DRAG_THRESHOLD
        });
        if crossed_threshold {
            self.sit_click = None;
        }
    }

    pub(super) fn finish_sit_click(&mut self, release_is_click: bool) -> Option<PhysicalPosition<f64>> {
        let click = self.sit_click.take()?;
        (release_is_click && click.travel[0] < CAMERA_DRAG_THRESHOLD && click.travel[1] < CAMERA_DRAG_THRESHOLD).then_some(click.position)
    }
}

struct SitClick {
    position: PhysicalPosition<f64>,
    travel: [f64; 2],
}

struct DragState {
    origin: PhysicalPosition<f64>,
    travel: [f64; 2],
}

impl SceneWindow {
    pub(in crate::gui::views::viewport) fn handle_scene_input(&mut self, event: &WindowEvent, capture: InputCapture) -> bool {
        self.synchronize_control_input();
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
                    self.sit_controls.handle_menu_event(SitMenuEvent::FocusLost);
                    self.release_drag(false);
                    self.modifiers = ModifiersState::empty();
                }
                self.synchronize_control_input();
                self.dirty = true;
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = *position,
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => self.handle_left_click(*state, capture),
            WindowEvent::MouseInput { button: MouseButton::Right, state: ElementState::Pressed, .. } => {
                self.input.cancel_sit_click();
                self.sit_controls.handle_right_click(
                    capture,
                    self.renderer.as_ref(),
                    self.feed.as_ref(),
                    self.last_camera,
                    self.cursor,
                    self.window.inner_size(),
                    self.dpi,
                );
                self.synchronize_control_input();
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
                    && self.sit_controls.menu_open() =>
            {
                self.sit_controls.handle_menu_event(SitMenuEvent::Escape);
                self.synchronize_control_input();
                self.dirty = true;
                return true;
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
                if self.sit_controls.menu_open()
                    && event.state == ElementState::Pressed
                    && matches!(KeyBinding::from_key(&event.logical_key), Some(KeyBinding::Movement(_)))
                {
                    self.sit_controls.handle_menu_event(SitMenuEvent::MovementKey);
                    self.synchronize_control_input();
                    self.dirty = true;
                    if capture.consumed || capture.keyboard {
                        return true;
                    }
                }
                let was_free = self.camera_mode == CameraMode::Free;
                let seated = self.feed.as_mut().is_some_and(SceneFeed::agent_is_seated);
                let sitting_input_active = self.feed.as_mut().is_some_and(SceneFeed::sitting_input_active);
                let reset_sitting_orbit = !was_free
                    && self.camera_mode == CameraMode::Follow
                    && should_reset_sitting_orbit(
                        &event.logical_key,
                        event.state,
                        event.repeat,
                        capture.consumed || capture.keyboard,
                        sitting_input_active,
                        self.modifiers.alt_key(),
                        self.modifiers.control_key(),
                        self.modifiers.shift_key(),
                    );
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
                    if reset_sitting_orbit {
                        self.reset_sitting_camera();
                    }
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

    pub(super) fn release_drag(&mut self, restore: bool) {
        self.input.cancel_sit_click();
        if let Some(drag) = self.input.drag.take() {
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
            let click_position = self.input.finish_sit_click(!capture.consumed && !capture.pointer);
            self.release_drag(true);
            if let Some(cursor) = click_position {
                self.sit_at_cursor(cursor);
            }
            return;
        }
        self.input.cancel_sit_click();
        if self.sit_controls.handle_left_click_while_menu_open(capture.consumed || capture.pointer) {
            self.synchronize_control_input();
            self.dirty = true;
            return;
        }
        if capture.consumed || capture.pointer {
            return;
        }

        if !self.modifiers.alt_key() {
            self.input.begin_sit_click(self.cursor, self.modifiers.is_empty());
            self.start_drag();
            return;
        }

        let point = {
            let (Some(renderer), Some(observation)) = (&self.renderer, self.last_camera) else {
                return;
            };
            let size = self.window.inner_size();
            match renderer.pick(observation, observation.ray([self.cursor.x, self.cursor.y], [size.width, size.height])) {
                Ok(hit) => hit.map(|hit| hit.position),
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

    fn sit_at_cursor(&mut self, cursor: PhysicalPosition<f64>) {
        let request =
            sit_request_for_left_click(self.renderer.as_ref(), self.last_camera, self.feed.as_mut(), cursor, self.window.inner_size());
        if self.sit_controls.start_if_requested(request, self.feed.as_ref(), std::sync::Arc::clone(&self.wake)) {
            self.egui.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
            self.dirty = true;
        }
    }

    pub(in crate::gui::views::viewport) fn drag_camera(&mut self, delta: (f64, f64)) {
        self.input.pointer_motion(delta, self.dpi);
        let Some(drag) = &mut self.input.drag else {
            return;
        };
        drag.travel[0] += delta.0.abs() / self.dpi;
        drag.travel[1] += delta.1.abs() / self.dpi;
        if drag.travel[0] >= CAMERA_DRAG_THRESHOLD || drag.travel[1] >= CAMERA_DRAG_THRESHOLD {
            self.dirty = true;
            let dy = if drag.travel[1] >= CAMERA_DRAG_THRESHOLD { delta.1 } else { 0.0 };
            let width = f64::from(self.window.inner_size().width);
            match self.camera_mode {
                CameraMode::Follow => {
                    let draw_distance = self.camera.as_ref().map_or(DEFAULT_DRAW_DISTANCE, |camera| camera.draw_distance);
                    self.follow_camera.drag([delta.0, dy], width, self.modifiers.control_key(), self.modifiers.shift_key(), draw_distance);
                }
                CameraMode::Free => {
                    if let Some(camera) = &mut self.camera {
                        camera.drag([delta.0, dy], width, self.modifiers.control_key(), self.modifiers.shift_key());
                    }
                }
            }
        }
    }

    fn start_drag(&mut self) {
        if self.input.drag.is_some() {
            return;
        }
        let origin = self.cursor;
        let size = self.window.inner_size();
        let center = PhysicalPosition::new(f64::from(size.width) * 0.5, f64::from(size.height) * 0.5);
        match self.window.set_cursor_grab(CursorGrabMode::Confined) {
            Ok(()) => {
                self.sit_controls.handle_menu_event(SitMenuEvent::CameraDragStarted);
                self.synchronize_control_input();
                self.window.set_cursor_visible(false);
                self.input.drag = Some(DragState { origin, travel: [0.0; 2] });
                if let Err(error) = self.window.set_cursor_position(center) {
                    self.release_drag(false);
                    tracing::warn!(%error, "could not position scene cursor");
                }
            }
            Err(error) => tracing::warn!(%error, "could not capture scene cursor"),
        }
    }
}
