use std::{
    f64::consts::{FRAC_PI_3, PI, TAU},
    time::{Duration, Instant},
};

use math::{DQuat, DVec3};
use render::scene;
use uuid::Uuid;
use world::WorldPose;

#[cfg(any(target_os = "windows", target_os = "linux"))]
use winit::event::ElementState;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use winit::keyboard::Key;

use crate::DEFAULT_DRAW_DISTANCE;
use crate::agent_movement::SittingCameraInput;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::agent_movement::{HeldControls, KeyBinding, MovementKey};
use crate::session::SitCameraOverride;

const FOCUS_TIME: Duration = Duration::from_millis(400);
const FOLLOW_SMOOTHING_HALF_LIFE: f64 = 0.02;
const MAX_FOLLOW_SMOOTH_DISTANCE: f64 = 50.0;

struct Transition {
    start: Instant,
    position: DVec3,
    focus: DVec3,
}

pub(crate) struct OrbitCamera {
    focus: DVec3,
    offset: DVec3,
    minimum: f64,
    transition: Option<Transition>,
    pub(crate) draw_distance: f32,
    pub(crate) lod_factor: f32,
}

pub(crate) struct FollowCamera {
    camera_offset: Option<DVec3>,
    updated_at: Option<Instant>,
    transition: Option<Transition>,
    focus_offset: DVec3,
    offset: DVec3,
    minimum: f64,
    sit_camera: Option<(Uuid, Option<SitCameraOverride>)>,
    last_target: Option<(DVec3, DVec3)>,
    sitting_camera_changed: bool,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            camera_offset: None,
            updated_at: None,
            transition: None,
            focus_offset: DVec3::new(1.0, 0.0, 1.0),
            offset: DVec3::new(-4.0, 0.0, -0.25),
            minimum: 0.5,
            sit_camera: None,
            last_target: None,
            sitting_camera_changed: false,
        }
    }
}

impl FollowCamera {
    pub(crate) fn follow_with_sit_input(
        &mut self,
        pose: WorldPose,
        sit_camera: Option<(Uuid, Option<SitCameraOverride>, WorldPose)>,
        sitting_input: SittingCameraInput,
        viewport: [u32; 2],
        draw_distance: f32,
        lod_factor: f32,
        now: Instant,
    ) -> Option<scene::Camera> {
        let current_sit_camera = sit_camera.map(|(object_id, camera, _)| (object_id, camera));
        if self.sit_camera != current_sit_camera {
            let (position, focus) = self.current_position_and_focus(pose, now);
            if current_sit_camera.is_some() {
                self.transition_from(position, focus, now);
            } else {
                self.transition = None;
            }
            self.sit_camera = current_sit_camera;
        }
        let elapsed = self.updated_at.map_or(Duration::ZERO, |updated| now.saturating_duration_since(updated));
        if sit_camera.is_none_or(|(_, camera, _)| camera.is_none()) {
            self.apply_sitting_camera_input(pose.rotation, sitting_input, elapsed);
        }
        let (target_position, target_focus) = match sit_camera {
            Some((_, Some(camera), seat_pose)) => (
                seat_pose.position + (seat_pose.rotation * camera.eye_offset).as_dvec3(),
                seat_pose.position + (seat_pose.rotation * camera.at_offset).as_dvec3(),
            ),
            Some((_, None, _)) | None => self.target_position_and_focus(pose),
        };
        let (position, focus) = self.advance_to(target_position, target_focus, now);
        Self::build_camera(position, focus, viewport, draw_distance, lod_factor)
    }

    fn build_camera(position: DVec3, focus: DVec3, viewport: [u32; 2], draw_distance: f32, lod_factor: f32) -> Option<scene::Camera> {
        scene::Camera::new(position, (focus - position).as_vec3(), FRAC_PI_3, viewport)
            .and_then(|camera| camera.with_quality(f64::from(draw_distance), f64::from(lod_factor)))
            .ok()
    }

    fn view_for_offsets(pose: WorldPose, focus_offset: DVec3, offset: DVec3) -> (DVec3, DVec3) {
        let focus = pose.position + (pose.rotation * focus_offset.as_vec3()).as_dvec3();
        let position = focus + (pose.rotation * offset.as_vec3()).as_dvec3();
        (position, focus)
    }

    fn target_position_and_focus(&self, pose: WorldPose) -> (DVec3, DVec3) {
        Self::view_for_offsets(pose, self.focus_offset, self.offset)
    }

    fn apply_sitting_camera_input(&mut self, rotation: math::Quat, input: SittingCameraInput, elapsed: Duration) {
        let dt = elapsed.as_secs_f64();
        if dt == 0.0 || input == SittingCameraInput::default() {
            return;
        }
        self.sitting_camera_changed = true;
        let mut offset = (rotation * self.offset.as_vec3()).as_dvec3();
        offset = DQuat::from_rotation_z(f64::from(input.around) * PI * 0.5 * dt) * offset;
        if input.over != 0.0 {
            let angle_from_up = offset.normalize().dot(DVec3::Z).clamp(-1.0, 1.0).acos();
            let next = (angle_from_up - f64::from(input.over) * PI * 0.5 * dt).clamp(PI / 180.0, PI * 179.0 / 180.0);
            let camera_left = DVec3::Z.cross(-offset.normalize()).normalize_or(DVec3::Y);
            offset = DQuat::from_axis_angle(camera_left, angle_from_up - next) * offset;
        }
        if input.distance != 0.0 {
            let distance = (offset.length() * (1.0 - f64::from(input.distance) * dt)).clamp(self.minimum, 240.0);
            offset = offset.normalize() * distance;
        }
        if input.pan_horizontal != 0.0 || input.pan_vertical != 0.0 {
            let direction = -offset.normalize();
            let left = DVec3::Z.cross(direction).normalize_or(DVec3::Y);
            let up = direction.cross(left);
            let pan = (left * f64::from(input.pan_horizontal) + up * f64::from(input.pan_vertical)) * (5.0 * dt);
            self.focus_offset += (rotation.conjugate() * pan.as_vec3()).as_dvec3();
        }
        self.offset = (rotation.conjugate() * offset.as_vec3()).as_dvec3();
    }

    pub(crate) fn reset_after_sitting(&mut self, pose: WorldPose, now: Instant) -> bool {
        if !self.sitting_camera_changed && self.sit_camera.is_none() {
            return false;
        }
        let (position, focus) = self.current_position_and_focus(pose, now);
        self.offset = DVec3::new(-4.0, 0.0, -0.25);
        self.focus_offset = DVec3::new(1.0, 0.0, 1.0);
        self.sitting_camera_changed = false;
        self.transition_from(position, focus, now);
        true
    }

    pub(crate) fn drag(&mut self, delta: [f64; 2], width: f64, control: bool, shift: bool, draw_distance: f32) {
        if width <= 0.0 {
            return;
        }
        let [dx, dy] = delta;
        let direction = -self.offset.normalize();
        let left = DVec3::Z.cross(direction).normalize_or(DVec3::Y);
        if control && shift {
            let up = direction.cross(left);
            self.focus_offset += (left * dx + up * dy) * (3.0 * self.offset.length() / width);
        } else {
            self.offset = DQuat::from_rotation_z(-dx * TAU / width) * self.offset;
            if control {
                let angle = self.offset.normalize().z.clamp(-1.0, 1.0).acos();
                let next = (angle - dy * TAU / width).clamp(PI / 180.0, PI * 179.0 / 180.0);
                let left = DVec3::Z.cross(-self.offset).normalize_or(DVec3::Y);
                self.offset = DQuat::from_axis_angle(left, angle - next) * self.offset;
            } else {
                self.zoom(0.99f64.powf(-dy), draw_distance);
            }
        }
    }

    pub(crate) fn zoom(&mut self, fraction: f64, draw_distance: f32) {
        let maximum = 240.0f64.min(f64::from(draw_distance) - 1.0);
        let distance = (self.offset.length() * fraction).clamp(self.minimum, maximum);
        self.offset = self.offset.normalize() * distance;
    }

    fn advance_to(&mut self, target_position: DVec3, target_focus: DVec3, now: Instant) -> (DVec3, DVec3) {
        self.last_target = Some((target_position, target_focus));
        let Some(previous_offset) = self.camera_offset else {
            self.camera_offset = Some(target_position - target_focus);
            self.updated_at = Some(now);
            return (target_position, target_focus);
        };

        let elapsed = self.updated_at.map_or(Duration::ZERO, |updated| now.saturating_duration_since(updated)).as_secs_f64();
        let fraction = 1.0 - 2.0_f64.powf(-elapsed / FOLLOW_SMOOTHING_HALF_LIFE);
        if let Some(transition) = &self.transition {
            let elapsed = now.saturating_duration_since(transition.start);
            if elapsed >= FOCUS_TIME {
                self.transition = None;
            } else {
                let fraction = elapsed.as_secs_f64() / FOCUS_TIME.as_secs_f64();
                let fraction = fraction * fraction * (3.0 - 2.0 * fraction);
                let position = transition.position.lerp(target_position, fraction);
                let focus = transition.focus.lerp(target_focus, fraction);
                self.camera_offset = Some(position - focus);
                self.updated_at = Some(now);
                return (position, focus);
            }
        }
        let target_offset = target_position - target_focus;
        let camera_offset = if target_offset.distance(previous_offset) >= MAX_FOLLOW_SMOOTH_DISTANCE {
            target_offset
        } else {
            previous_offset.lerp(target_offset, fraction)
        };
        self.camera_offset = Some(camera_offset);
        self.updated_at = Some(now);
        (target_focus + camera_offset, target_focus)
    }

    pub(crate) fn current_position_and_focus(&self, pose: WorldPose, now: Instant) -> (DVec3, DVec3) {
        let (target_position, target_focus) = self.last_target.unwrap_or_else(|| self.target_position_and_focus(pose));
        if let Some(transition) = &self.transition {
            let elapsed = now.saturating_duration_since(transition.start);
            if elapsed < FOCUS_TIME {
                let fraction = elapsed.as_secs_f64() / FOCUS_TIME.as_secs_f64();
                let fraction = fraction * fraction * (3.0 - 2.0 * fraction);
                return (transition.position.lerp(target_position, fraction), transition.focus.lerp(target_focus, fraction));
            }
        }
        self.camera_offset.map_or((target_position, target_focus), |offset| (target_focus + offset, target_focus))
    }

    pub(crate) fn moving(&self, pose: WorldPose, now: Instant) -> bool {
        if self.transition.as_ref().is_some_and(|transition| now.saturating_duration_since(transition.start) < FOCUS_TIME) {
            return true;
        }
        let (target_position, target_focus) = self.last_target.unwrap_or_else(|| self.target_position_and_focus(pose));
        self.camera_offset.is_some_and(|offset| (target_position - target_focus).distance(offset) > 0.01)
    }

    pub(crate) fn transition_from(&mut self, position: DVec3, focus: DVec3, now: Instant) {
        self.camera_offset = Some(position - focus);
        self.updated_at = Some(now);
        self.transition = Some(Transition { start: now, position, focus });
        self.last_target = Some((position, focus));
    }
}

impl OrbitCamera {
    pub(crate) fn moving(&self, now: Instant) -> bool {
        self.transition.as_ref().is_some_and(|transition| now.saturating_duration_since(transition.start) < FOCUS_TIME)
    }
    pub(crate) fn new(position: DVec3) -> Self {
        Self {
            focus: position,
            offset: DVec3::new(-4.0, 0.0, 2.0),
            minimum: 0.5,
            transition: None,
            draw_distance: DEFAULT_DRAW_DISTANCE,
            lod_factor: 1.0,
        }
    }

    pub(crate) fn from_follow_view(position: DVec3, focus: DVec3, draw_distance: f32, lod_factor: f32) -> Self {
        Self { focus, offset: position - focus, minimum: 0.5, transition: None, draw_distance, lod_factor }
    }

    pub(crate) fn scene_camera(&self, viewport: [u32; 2], now: Instant) -> Option<scene::Camera> {
        let (position, focus) = self.position_and_focus(now);
        scene::Camera::new(position, (focus - position).as_vec3(), FRAC_PI_3, viewport)
            .and_then(|camera| camera.with_quality(f64::from(self.draw_distance), f64::from(self.lod_factor)))
            .ok()
    }

    fn position_and_focus(&self, now: Instant) -> (DVec3, DVec3) {
        let position = self.focus + self.offset;
        let Some(transition) = &self.transition else {
            return (position, self.focus);
        };
        let time = now.saturating_duration_since(transition.start).as_secs_f64() / FOCUS_TIME.as_secs_f64();
        if time >= 1.0 {
            return (position, self.focus);
        }
        let fraction = time * time * (3.0 - 2.0 * time);
        (transition.position.lerp(position, fraction), transition.focus.lerp(self.focus, fraction))
    }

    pub(crate) fn current_position_and_focus(&self, now: Instant) -> (DVec3, DVec3) {
        self.position_and_focus(now)
    }

    pub(crate) fn focus(&mut self, point: DVec3, now: Instant) {
        let (position, focus) = self.position_and_focus(now);
        let offset = position - point;
        if point.is_finite() && offset.length() >= 0.02 {
            self.transition = Some(Transition { start: now, position, focus });
            self.focus = point;
            self.offset = offset;
            self.minimum = 0.02;
        }
    }

    pub(crate) fn drag(&mut self, delta: [f64; 2], width: f64, control: bool, shift: bool) {
        if width <= 0.0 {
            return;
        }
        let [dx, dy] = delta;
        let direction = -self.offset.normalize();
        let left = DVec3::Z.cross(direction).normalize_or(DVec3::Y);
        if control && shift {
            let up = direction.cross(left);
            self.focus += (left * dx + up * dy) * (3.0 * self.offset.length() / width);
        } else {
            self.offset = DQuat::from_rotation_z(-dx * TAU / width) * self.offset;
            if control {
                let angle = self.offset.normalize().z.clamp(-1.0, 1.0).acos();
                let next = (angle - dy * TAU / width).clamp(PI / 180.0, PI * 179.0 / 180.0);
                let left = DVec3::Z.cross(-self.offset).normalize_or(DVec3::Y);
                self.offset = DQuat::from_axis_angle(left, angle - next) * self.offset;
            } else {
                self.zoom(0.99f64.powf(-dy));
            }
        }
    }

    pub(crate) fn zoom(&mut self, fraction: f64) {
        let distance = (self.offset.length() * fraction).clamp(self.minimum, 240.0f64.min(f64::from(self.draw_distance) - 1.0));
        self.offset = self.offset.normalize() * distance;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CameraMode {
    Follow,
    Free,
}

impl CameraMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Follow => "Follow",
            Self::Free => "Free",
        }
    }

    pub(crate) fn detach_for_focus(&mut self) {
        *self = Self::Free;
    }

    pub(crate) fn return_to_follow(&mut self) {
        *self = Self::Follow;
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn on_movement_key(&mut self, binding: KeyBinding, state: ElementState, repeat: bool, captured: bool) {
        if state == ElementState::Pressed
            && !repeat
            && !captured
            && matches!(binding, KeyBinding::Movement(_) | KeyBinding::ToggleFlying | KeyBinding::ToggleRun | KeyBinding::Stop)
        {
            self.return_to_follow();
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn should_reset_sitting_orbit(
    key: &Key,
    state: ElementState,
    repeat: bool,
    captured: bool,
    seated: bool,
    alt: bool,
    control: bool,
    shift: bool,
) -> bool {
    state == ElementState::Pressed
        && !repeat
        && !captured
        && !seated
        && !alt
        && !control
        && KeyBinding::from_key(key).is_some_and(|binding| match binding {
            KeyBinding::Movement(MovementKey::Forward | MovementKey::ForwardArrow | MovementKey::Backward | MovementKey::BackwardArrow) => {
                true
            }
            KeyBinding::Movement(
                MovementKey::TurnLeft | MovementKey::TurnLeftArrow | MovementKey::TurnRight | MovementKey::TurnRightArrow,
            ) => shift,
            _ => false,
        })
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn handle_keyboard_input(
    controls: &mut HeldControls,
    camera_mode: &mut CameraMode,
    key: &Key,
    state: ElementState,
    repeat: bool,
    captured: bool,
    seated: bool,
) -> Option<bool> {
    if state == ElementState::Pressed && !repeat && KeyBinding::from_key(key).is_some() {
        tracing::debug!(?key, captured, "viewport movement key received");
    }
    let (binding, changed) = controls.keyboard_event(key, state, repeat, captured, seated)?;
    camera_mode.on_movement_key(binding, state, repeat, captured);
    Some(changed)
}

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;
