use std::{
    f64::consts::{FRAC_PI_3, PI, TAU},
    time::{Duration, Instant},
};

#[cfg(test)]
use math::Vec3;
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
use crate::agent_movement::{HeldControls, KeyBinding};
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
        }
    }
}

impl FollowCamera {
    #[cfg(test)]
    pub(crate) fn scene_camera(pose: WorldPose, viewport: [u32; 2], draw_distance: f32, lod_factor: f32) -> Option<scene::Camera> {
        let (position, focus) = Self::position_and_focus(pose);
        Self::build_camera(position, focus, viewport, draw_distance, lod_factor)
    }

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

    #[cfg(test)]
    fn follow_with_sit_camera(
        &mut self,
        pose: WorldPose,
        sit_camera: Option<(Uuid, SitCameraOverride, WorldPose)>,
        viewport: [u32; 2],
        draw_distance: f32,
        lod_factor: f32,
        now: Instant,
    ) -> Option<scene::Camera> {
        self.follow_with_sit_input(
            pose,
            sit_camera.map(|(id, camera, pose)| (id, Some(camera), pose)),
            SittingCameraInput::default(),
            viewport,
            draw_distance,
            lod_factor,
            now,
        )
    }

    fn build_camera(position: DVec3, focus: DVec3, viewport: [u32; 2], draw_distance: f32, lod_factor: f32) -> Option<scene::Camera> {
        scene::Camera::new(position, (focus - position).as_vec3(), FRAC_PI_3, viewport)
            .and_then(|camera| camera.with_quality(f64::from(draw_distance), f64::from(lod_factor)))
            .ok()
    }

    #[cfg(test)]
    fn position_and_focus(pose: WorldPose) -> (DVec3, DVec3) {
        Self::view_for_offsets(pose, DVec3::new(1.0, 0.0, 1.0), DVec3::new(-4.0, 0.0, -0.25))
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

    #[cfg(test)]
    pub(crate) fn from_follow_pose(pose: WorldPose, draw_distance: f32, lod_factor: f32) -> Self {
        let (position, focus) = FollowCamera::position_and_focus(pose);
        Self { focus, offset: position - focus, minimum: 0.5, transition: None, draw_distance, lod_factor }
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
mod tests {
    use super::*;
    use math::Quat;

    #[test]
    fn follow_camera_uses_official_rear_view_offsets_in_agent_axes() {
        let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
        let (position, focus) = FollowCamera::position_and_focus(pose);
        assert_eq!(position, pose.position + DVec3::new(-3.0, 0.0, 0.75));
        assert_eq!(focus, pose.position + DVec3::new(1.0, 0.0, 1.0));

        let pose = WorldPose { rotation: math::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..pose };
        let (position, focus) = FollowCamera::position_and_focus(pose);
        assert!((position - (pose.position + DVec3::new(0.0, -3.0, 0.75))).length() < 1e-6);
        assert!((focus - (pose.position + DVec3::new(0.0, 1.0, 1.0))).length() < 1e-6);
    }

    #[test]
    fn follow_camera_moves_rigidly_with_avatar_translation() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::new(10.0, 20.0, 30.0), rotation: math::Quat::IDENTITY };
        let mut camera = FollowCamera::default();
        let first = camera.follow_with_sit_camera(pose, None, [1280, 720], 512.0, 1.0, start).expect("initial follow camera");
        let translated = WorldPose { position: pose.position + DVec3::new(100.0, -50.0, 20.0), ..pose };
        let second = camera
            .follow_with_sit_camera(translated, None, [1280, 720], 512.0, 1.0, start + Duration::from_millis(10))
            .expect("translated follow camera");

        assert_eq!(second.position() - first.position(), DVec3::new(100.0, -50.0, 20.0));
        assert!(!camera.moving(translated, start + Duration::from_millis(10)));
    }

    #[test]
    fn follow_camera_converges_offset_after_turn() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::ZERO, rotation: math::Quat::IDENTITY };
        let mut camera = FollowCamera::default();
        camera.follow_with_sit_camera(pose, None, [1280, 720], 512.0, 1.0, start).expect("initial follow camera");
        let turned = WorldPose { rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..pose };
        let halfway = camera
            .follow_with_sit_camera(turned, None, [1280, 720], 512.0, 1.0, start + Duration::from_millis(20))
            .expect("turned follow camera");
        let (target, target_focus) = FollowCamera::position_and_focus(turned);
        assert!(halfway.position().distance(target) > 0.01, "camera offset should lag the avatar turn");
        let (_, focus) = camera.current_position_and_focus(turned, start + Duration::from_millis(20));
        assert_eq!(focus, target_focus);

        let converged = camera
            .follow_with_sit_camera(turned, None, [1280, 720], 512.0, 1.0, start + Duration::from_secs(1))
            .expect("converged follow camera");
        assert!(converged.position().distance(target) < 0.01);
        assert!(!camera.moving(turned, start + Duration::from_secs(1)));
    }

    #[test]
    fn follow_camera_snaps_large_offset_changes() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::ZERO, rotation: math::Quat::IDENTITY };
        let mut camera = FollowCamera::default();
        camera.follow_with_sit_camera(pose, None, [1280, 720], 512.0, 1.0, start).expect("initial follow camera");
        camera.offset = DVec3::new(-100.0, 0.0, 0.0);
        let (target, _) = camera.target_position_and_focus(pose);

        let snapped = camera
            .follow_with_sit_camera(pose, None, [1280, 720], 512.0, 1.0, start + Duration::from_millis(10))
            .expect("snapped follow camera");
        assert!(snapped.position().distance(target) < 1e-9);
        assert!(!camera.moving(pose, start + Duration::from_millis(10)));
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn picked_focus_detaches_and_escape_returns_to_follow() {
        let mut mode = CameraMode::Follow;
        mode.detach_for_focus();
        assert_eq!(mode, CameraMode::Free);

        mode.return_to_follow();
        assert_eq!(mode, CameraMode::Follow);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn uncaptured_movement_returns_to_follow() {
        let mut mode = CameraMode::Free;
        let mut controls = HeldControls::default();

        assert_eq!(
            handle_keyboard_input(&mut controls, &mut mode, &Key::Character("w".into()), ElementState::Pressed, false, false, false),
            Some(true)
        );
        assert_eq!(mode, CameraMode::Follow);
        assert_eq!(controls.observation(false, math::Quat::IDENTITY).control_flags & 0x0000_0001, 0x0000_0001);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn egui_captured_movement_does_not_return_to_follow() {
        let mut mode = CameraMode::Free;
        let mut controls = HeldControls::default();

        assert_eq!(
            handle_keyboard_input(&mut controls, &mut mode, &Key::Character("w".into()), ElementState::Pressed, false, true, false),
            None
        );
        assert_eq!(mode, CameraMode::Free);
        assert_eq!(controls.observation(false, math::Quat::IDENTITY).control_flags, 0);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn flight_run_and_stop_keys_return_to_follow() {
        use winit::keyboard::NamedKey;

        for key in [Key::Character("f".into()), Key::Character("r".into()), Key::Named(NamedKey::Space)] {
            let mut mode = CameraMode::Free;
            let mut controls = HeldControls::default();

            let binding = KeyBinding::from_key(&key).expect("mapped camera-return key");
            assert_eq!(handle_keyboard_input(&mut controls, &mut mode, &key, ElementState::Pressed, false, false, false), Some(true));
            assert!(matches!(binding, KeyBinding::ToggleFlying | KeyBinding::ToggleRun | KeyBinding::Stop));
            assert_eq!(mode, CameraMode::Follow);
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn movement_key_release_does_not_change_camera_mode() {
        use crate::agent_movement::MovementKey;

        let mut mode = CameraMode::Free;
        let mut controls = HeldControls::default();
        controls.set_pressed(MovementKey::Forward, true);

        assert_eq!(
            handle_keyboard_input(&mut controls, &mut mode, &Key::Character("w".into()), ElementState::Released, false, false, false),
            Some(true)
        );
        assert_eq!(mode, CameraMode::Free);
    }

    #[test]
    fn detaching_from_follow_preserves_the_live_view() {
        let pose = WorldPose { position: DVec3::new(1280.0, -256.0, 45.0), rotation: math::Quat::from_rotation_z(0.7) };
        let now = Instant::now();
        let follow = FollowCamera::scene_camera(pose, [1280, 720], 512.0, 1.5).expect("valid follow camera");
        let free = OrbitCamera::from_follow_pose(pose, 512.0, 1.5).scene_camera([1280, 720], now).expect("valid free camera");

        assert!(free.position().distance(follow.position()) < 1e-9);
        assert!(free.direction().distance(follow.direction()) < 1e-6);
        assert_eq!(free.draw_distance(), follow.draw_distance());
    }

    #[test]
    fn sit_camera_overrides_follow_the_world_resolved_seat_pose() {
        let pose =
            WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) };
        let eye = Vec3::new(2.0, 0.0, 1.0);
        let at = Vec3::new(0.0, 0.0, 1.0);
        let mut follow = FollowCamera::default();
        let now = Instant::now();
        let sit_camera = SitCameraOverride { eye_offset: eye, at_offset: at };
        let seat_id = Uuid::from_u128(1);
        follow.follow_with_sit_camera(pose, Some((seat_id, sit_camera, pose)), [1280, 720], 128.0, 1.0, now).expect("entering sit camera");
        let camera = follow
            .follow_with_sit_camera(
                pose,
                Some((seat_id, sit_camera, pose)),
                [1280, 720],
                128.0,
                1.0,
                now + FOCUS_TIME + Duration::from_secs(1),
            )
            .expect("settled sit camera");

        assert!((camera.position() - (pose.position + DVec3::new(0.0, 2.0, 1.0))).length() < 1e-6);
        assert!(camera.direction().distance(Vec3::new(0.0, -1.0, 0.0)) < 1e-6);
    }

    #[test]
    fn seated_camera_without_offsets_uses_follow_view() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::new(40.0, 50.0, 60.0), rotation: Quat::from_rotation_z(0.4) };
        let seat_id = Uuid::from_u128(12);
        let mut camera = FollowCamera::default();
        camera
            .follow_with_sit_input(pose, Some((seat_id, None, pose)), SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start)
            .expect("seated default camera transition");
        let seated = camera
            .follow_with_sit_input(
                pose,
                Some((seat_id, None, pose)),
                SittingCameraInput::default(),
                [1280, 720],
                128.0,
                1.0,
                start + FOCUS_TIME,
            )
            .expect("settled seated default camera");
        let (position, focus) = FollowCamera::position_and_focus(pose);
        assert!(seated.position().distance(position) < 1e-6);
        assert!(seated.direction().distance((focus - position).as_vec3().normalize()) < 1e-6);
    }

    #[test]
    fn offset_camera_tracks_moving_rotated_seat() {
        let start = Instant::now();
        let first = WorldPose { position: DVec3::new(40.0, 50.0, 60.0), rotation: Quat::IDENTITY };
        let second = WorldPose { position: DVec3::new(44.0, 52.0, 61.0), rotation: Quat::from_rotation_z(0.8) };
        let seat_id = Uuid::from_u128(13);
        let camera_override = SitCameraOverride { eye_offset: Vec3::new(2.0, -1.0, 0.5), at_offset: Vec3::new(0.5, 0.0, 1.0) };
        let mut camera = FollowCamera::default();
        camera
            .follow_with_sit_input(
                first,
                Some((seat_id, Some(camera_override), first)),
                SittingCameraInput::default(),
                [1280, 720],
                128.0,
                1.0,
                start,
            )
            .expect("initial seat camera");
        let moving = camera
            .follow_with_sit_input(
                second,
                Some((seat_id, Some(camera_override), second)),
                SittingCameraInput::default(),
                [1280, 720],
                128.0,
                1.0,
                start + FOCUS_TIME + Duration::from_secs(1),
            )
            .expect("camera after seat movement");
        let expected_position = second.position + (second.rotation * camera_override.eye_offset).as_dvec3();
        let expected_focus = second.position + (second.rotation * camera_override.at_offset).as_dvec3();
        assert!(moving.position().distance(expected_position) < 1e-6);
        assert!(moving.direction().distance((expected_focus - expected_position).as_vec3().normalize()) < 1e-6);
    }

    #[test]
    fn sit_camera_entry_animates_and_stand_uses_follow_smoothing() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
        let sit_pose = WorldPose { position: pose.position + DVec3::new(2.0, 0.0, 1.0), ..pose };
        let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
        let seat_id = Uuid::from_u128(1);
        let transition_start = start + Duration::from_millis(1);
        let mut camera = FollowCamera::default();
        let standing = camera.follow_with_sit_camera(pose, None, [1280, 720], 128.0, 1.0, start).expect("standing camera");
        let entering = camera
            .follow_with_sit_camera(sit_pose, Some((seat_id, sit_camera, sit_pose)), [1280, 720], 128.0, 1.0, transition_start)
            .expect("camera while entering the seat");
        assert_eq!(entering.position(), standing.position());
        assert_eq!(entering.direction(), standing.direction());

        let seated_at = transition_start + FOCUS_TIME;
        let seated = camera
            .follow_with_sit_camera(sit_pose, Some((seat_id, sit_camera, sit_pose)), [1280, 720], 128.0, 1.0, seated_at)
            .expect("seated camera");
        let leaving = camera
            .follow_with_sit_camera(pose, None, [1280, 720], 128.0, 1.0, seated_at + Duration::from_millis(1))
            .expect("camera while leaving the seat");
        assert_ne!(leaving.position(), seated.position());
        assert!(camera.moving(pose, seated_at + Duration::from_millis(1)));

        let standing_again = camera
            .follow_with_sit_camera(pose, None, [1280, 720], 128.0, 1.0, seated_at + Duration::from_millis(200))
            .expect("standing camera after follow smoothing");
        let expected = FollowCamera::scene_camera(pose, [1280, 720], 128.0, 1.0).expect("default follow camera");
        assert!(standing_again.position().distance(expected.position()) < 0.1);
        assert!(standing_again.direction().distance(expected.direction()) < 0.02);
        assert!(!camera.moving(pose, seated_at + Duration::from_millis(200)));
    }

    #[test]
    fn switching_seats_with_same_camera_offsets_transitions() {
        let start = Instant::now();
        let first_pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
        let second_pose = WorldPose { position: DVec3::new(180.0, 40.0, 22.0), rotation: math::Quat::IDENTITY };
        let first_seat = Uuid::from_u128(1);
        let second_seat = Uuid::from_u128(2);
        let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
        let mut camera = FollowCamera::default();
        camera
            .follow_with_sit_camera(first_pose, Some((first_seat, sit_camera, first_pose)), [1280, 720], 128.0, 1.0, start)
            .expect("first seat camera");
        let seated = camera
            .follow_with_sit_camera(
                first_pose,
                Some((first_seat, sit_camera, first_pose)),
                [1280, 720],
                128.0,
                1.0,
                start + FOCUS_TIME + Duration::from_secs(1),
            )
            .expect("settled first seat camera");
        let switch_at = start + FOCUS_TIME + Duration::from_secs(1) + Duration::from_millis(1);
        let switching = camera
            .follow_with_sit_camera(second_pose, Some((second_seat, sit_camera, second_pose)), [1280, 720], 128.0, 1.0, switch_at)
            .expect("camera at seat switch");
        assert_eq!(switching.position(), seated.position());
        assert_eq!(switching.direction(), seated.direction());

        let settled = camera
            .follow_with_sit_camera(
                second_pose,
                Some((second_seat, sit_camera, second_pose)),
                [1280, 720],
                128.0,
                1.0,
                switch_at + FOCUS_TIME,
            )
            .expect("settled second seat camera");
        assert!((settled.position() - (second_pose.position + DVec3::new(2.0, 0.0, 1.0))).length() < 1e-6);
        assert!(settled.direction().distance(Vec3::new(-1.0, 0.0, 0.0)) < 1e-6);
    }

    #[test]
    fn returning_from_free_camera_preserves_live_view_when_sitting() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::new(100.0, 200.0, 30.0), rotation: math::Quat::IDENTITY };
        let sit_pose = WorldPose { position: pose.position + DVec3::new(2.0, 0.0, 1.0), ..pose };
        let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
        let seat_id = Uuid::from_u128(1);
        let mut camera = FollowCamera::default();
        camera.follow_with_sit_camera(pose, None, [1280, 720], 128.0, 1.0, start).expect("standing camera");

        let free_position = pose.position + DVec3::new(-8.0, 2.0, 4.0);
        let free_focus = pose.position + DVec3::new(1.0, 1.0, 1.0);
        let returned_at = start + Duration::from_millis(1);
        camera.transition_from(free_position, free_focus, returned_at);
        let returned = camera
            .follow_with_sit_camera(
                pose,
                Some((seat_id, sit_camera, sit_pose)),
                [1280, 720],
                128.0,
                1.0,
                returned_at + Duration::from_millis(1),
            )
            .expect("follow camera after returning from free mode");

        assert_eq!(returned.position(), free_position);
        assert!(returned.direction().distance((free_focus - free_position).as_vec3().normalize()) < 1e-6);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn ground_sitting_keys_orbit_the_camera() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation: Quat::IDENTITY };
        let mut controls = HeldControls::default();
        for key in ["a", "w", "e"] {
            controls.keyboard_event(&Key::Character(key.into()), ElementState::Pressed, false, false, true).expect("sitting key");
        }
        let actions = controls.sitting_actions(
            false,
            false,
            false,
            false,
            crate::agent_movement::ScriptControlGrabs::default(),
            start + Duration::from_secs(1),
        );
        assert_eq!(actions.control_flags, 0);
        assert_eq!(actions.camera.around, -1.0);
        assert_eq!(actions.camera.over, 1.0);
        assert_eq!(actions.camera.distance, 1.0);

        let mut camera = FollowCamera::default();
        let before = camera
            .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start)
            .expect("ground sit camera before input");
        let after = camera
            .follow_with_sit_input(pose, None, actions.camera, [1280, 720], 128.0, 1.0, start + Duration::from_secs(1))
            .expect("ground sit camera after input");
        assert!(after.position().distance(before.position()) > 1.0);
        assert!(after.direction().distance(before.direction()) > 0.1);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn object_sitting_without_offsets_orbits_the_camera() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation: Quat::IDENTITY };
        let seat_id = Uuid::from_u128(10);
        let mut controls = HeldControls::default();
        for key in ["d", "s", "c"] {
            controls.keyboard_event(&Key::Character(key.into()), ElementState::Pressed, false, false, true).expect("sitting key");
        }
        let actions = controls.sitting_actions(
            false,
            false,
            false,
            false,
            crate::agent_movement::ScriptControlGrabs::default(),
            start + Duration::from_secs(1),
        );
        assert_eq!(actions.control_flags, 0);
        assert_eq!(actions.camera.around, 1.0);
        assert_eq!(actions.camera.over, -1.0);
        assert_eq!(actions.camera.distance, -1.0);

        let mut camera = FollowCamera::default();
        camera
            .follow_with_sit_input(pose, Some((seat_id, None, pose)), SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start)
            .expect("object sit camera before input");
        let before = camera
            .follow_with_sit_input(
                pose,
                Some((seat_id, None, pose)),
                SittingCameraInput::default(),
                [1280, 720],
                128.0,
                1.0,
                start + FOCUS_TIME,
            )
            .expect("object sit camera after entry transition");
        let after = camera
            .follow_with_sit_input(
                pose,
                Some((seat_id, None, pose)),
                actions.camera,
                [1280, 720],
                128.0,
                1.0,
                start + FOCUS_TIME + Duration::from_secs(1),
            )
            .expect("object sit camera after input");
        assert!(after.position().distance(before.position()) > 1.0);
        assert!(after.direction().distance(before.direction()) > 0.1);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn object_sit_camera_forwards_sitting_keys() {
        let now = Instant::now();
        let pose = WorldPose { position: DVec3::new(20.0, 30.0, 40.0), rotation: Quat::IDENTITY };
        let seat_id = Uuid::from_u128(11);
        let sit_camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
        let mut controls = HeldControls::default();
        for key in ["a", "w", "e"] {
            controls.keyboard_event(&Key::Character(key.into()), ElementState::Pressed, false, false, true).expect("sitting key");
        }
        let actions = controls.sitting_actions(
            false,
            false,
            false,
            true,
            crate::agent_movement::ScriptControlGrabs::default(),
            now + Duration::from_secs(1),
        );
        assert_eq!(actions.camera, SittingCameraInput::default());
        assert_eq!(actions.control_flags, 0x0000_1511);

        let mut camera = FollowCamera::default();
        camera
            .follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, now)
            .expect("standing camera before sit");
        camera
            .follow_with_sit_input(
                pose,
                Some((seat_id, Some(sit_camera), pose)),
                SittingCameraInput::default(),
                [1280, 720],
                128.0,
                1.0,
                now + Duration::from_millis(1),
            )
            .expect("sit camera transition");
        let camera = camera
            .follow_with_sit_input(
                pose,
                Some((seat_id, Some(sit_camera), pose)),
                actions.camera,
                [1280, 720],
                128.0,
                1.0,
                now + FOCUS_TIME + Duration::from_secs(1),
            )
            .expect("sit camera with control input");
        assert_eq!(camera.position(), pose.position + DVec3::new(2.0, 0.0, 1.0));
        assert_eq!(camera.direction(), Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn sitting_orbit_rate_matches_viewer_camera_rate() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::ZERO, rotation: Quat::IDENTITY };
        let input = SittingCameraInput { around: 1.0, ..SittingCameraInput::default() };
        let mut camera = FollowCamera::default();
        camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start).expect("initial camera");
        let orbit = camera
            .follow_with_sit_input(pose, None, input, [1280, 720], 128.0, 1.0, start + Duration::from_secs(1))
            .expect("camera after orbit");
        assert!((orbit.position() - DVec3::new(1.0, -4.0, 0.75)).length() < 0.001);
    }

    #[test]
    fn sitting_under_orbit_clamps_to_viewer_limit() {
        let start = Instant::now();
        let pose = WorldPose { position: DVec3::ZERO, rotation: Quat::IDENTITY };
        let input = SittingCameraInput { over: -1.0, ..SittingCameraInput::default() };
        let mut camera = FollowCamera::default();
        camera.follow_with_sit_input(pose, None, SittingCameraInput::default(), [1280, 720], 128.0, 1.0, start).expect("initial camera");
        let result = camera
            .follow_with_sit_input(pose, None, input, [1280, 720], 128.0, 1.0, start + Duration::from_secs(1))
            .expect("camera after under orbit");
        let focus = pose.position + DVec3::new(1.0, 0.0, 1.0);
        let offset = (result.position() - focus).normalize();
        assert!((offset.dot(DVec3::Z) - (179.0 * PI / 180.0).cos()).abs() < 1e-6);
    }

    #[test]
    fn focus_eases() {
        let now = Instant::now();
        let mut camera = OrbitCamera::new(DVec3::ZERO);
        let before = camera.scene_camera([1280, 720], now).unwrap();
        let point = DVec3::new(10.0, 5.0, 0.0);
        assert!(!camera.moving(now));
        camera.focus(point, now);
        assert!(camera.moving(now));
        let after = camera.scene_camera([1280, 720], now).unwrap();
        assert!(before.direction().distance(after.direction()) < 1e-6, "selecting focus must not snap the view");
        let halfway = now + FOCUS_TIME / 2;
        let (position, focus) = camera.position_and_focus(halfway);
        assert_eq!(focus, point * 0.5);
        assert_eq!(position, before.position());
        let end = camera.scene_camera([1280, 720], now + FOCUS_TIME).unwrap();
        assert!(!camera.moving(now + FOCUS_TIME), "redraws stop at the end of the focus transition");
        assert!(end.direction().distance((point - position).normalize().as_vec3()) < 1e-6);
        assert_eq!(camera.position_and_focus(now + FOCUS_TIME * 2).1, point);

        let visible = camera.scene_camera([1280, 720], halfway).unwrap();
        camera.focus(DVec3::new(20.0, -5.0, 3.0), halfway);
        let interrupted = camera.scene_camera([1280, 720], halfway).unwrap();
        assert_eq!(visible.position(), interrupted.position());
        assert!(visible.direction().distance(interrupted.direction()) < 1e-6);
        camera.drag([40.0, 0.0], 1000.0, true, false);
        assert!(camera.scene_camera([1280, 720], halfway + FOCUS_TIME / 2).unwrap().position().distance(position) > 0.1);
    }

    #[test]
    fn focus_keeps_position() {
        let home = DVec3::new(1e9, 2e9, 50.0);
        let now = Instant::now();
        let mut camera = OrbitCamera::new(home);
        let position = camera.scene_camera([1280, 720], now).unwrap().position();
        camera.focus(home + DVec3::new(10.0, 2.0, 3.0), now);
        for elapsed in [Duration::ZERO, FOCUS_TIME / 2, FOCUS_TIME] {
            assert!(camera.scene_camera([1280, 720], now + elapsed).unwrap().position().distance(position) < 1e-6);
        }
        assert_eq!(camera.focus, home + DVec3::new(10.0, 2.0, 3.0));
    }

    #[test]
    fn orbit_and_pan() {
        let mut camera = OrbitCamera::new(DVec3::ZERO);
        camera.drag([250.0, 0.0], 1000.0, true, false);
        assert!(camera.offset.distance(DVec3::new(0.0, 4.0, 2.0)) < 1e-10);
        assert_eq!(camera.focus, DVec3::ZERO);
        let offset = camera.offset;
        camera.drag([100.0, 0.0], 1000.0, true, true);
        assert_eq!(camera.offset, offset);
        assert!(camera.focus.distance(DVec3::new(20.0f64.sqrt() * 0.3, 0.0, 0.0)) < 1e-10);
        camera.drag([0.0, 1000.0], 1000.0, true, false);
        assert!((camera.offset.normalize().z.acos() - PI / 180.0).abs() < 1e-10);
    }

    #[test]
    fn zoom_limits() {
        let mut camera = OrbitCamera::new(DVec3::ZERO);
        camera.zoom(0.0001);
        assert!((camera.offset.length() - 0.5).abs() < 1e-10);
        camera.focus(DVec3::X, Instant::now());
        camera.zoom(0.0001);
        assert!((camera.offset.length() - 0.02).abs() < 1e-10);
        camera.zoom(1e6);
        assert!((camera.offset.length() - 240.0).abs() < 1e-10);
        camera.draw_distance = 32.0;
        camera.lod_factor = 2.0;
        camera.zoom(1e6);
        assert!((camera.offset.length() - 31.0).abs() < 1e-10);
    }
}
