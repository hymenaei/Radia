#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::time::{Duration, Instant};

use math::{Quat, Vec3};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use winit::{
    event::ElementState,
    keyboard::{Key, NamedKey},
};

const AT_POS: u32 = 0x0000_0001;
const AT_NEG: u32 = 0x0000_0002;
const LEFT_POS: u32 = 0x0000_0004;
const LEFT_NEG: u32 = 0x0000_0008;
const UP_POS: u32 = 0x0000_0010;
const UP_NEG: u32 = 0x0000_0020;
const YAW_POS: u32 = 0x0000_0100;
const YAW_NEG: u32 = 0x0000_0200;
const FAST_AT: u32 = 0x0000_0400;
const FAST_LEFT: u32 = 0x0000_0800;
const FAST_UP: u32 = 0x0000_1000;
const FLY: u32 = 0x0000_2000;
pub(crate) const STOP: u32 = 0x0000_4000;
const SIT_ON_GROUND: u32 = 0x0002_0000;
const STAND_UP: u32 = 0x0001_0000;
pub(crate) const BODY_ROTATION_DOT_THRESHOLD: f32 = 0.9997;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const TURN_RATE: f32 = std::f32::consts::FRAC_PI_2;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const MAX_TURN_STEP: Duration = Duration::from_millis(100);

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MovementKey {
    Forward,
    ForwardArrow,
    Backward,
    BackwardArrow,
    TurnLeft,
    TurnLeftArrow,
    TurnRight,
    TurnRightArrow,
    Jump,
    JumpPage,
    Down,
    DownPage,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyBinding {
    Movement(MovementKey),
    ToggleFlying,
    ToggleRun,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MovementObservation {
    pub(crate) control_flags: u32,
    pub(crate) body_rotation: Quat,
    pub(crate) always_run: bool,
    pub(crate) stop_sequence: u64,
}

impl Default for MovementObservation {
    fn default() -> Self {
        Self { control_flags: 0, body_rotation: Quat::IDENTITY, always_run: false, stop_sequence: 0 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MovementAction {
    SitOnGround,
    StandUp,
}

impl MovementAction {
    pub(crate) const fn control_flag(self) -> u32 {
        match self {
            Self::SitOnGround => SIT_ON_GROUND,
            Self::StandUp => STAND_UP,
        }
    }
}

impl MovementObservation {
    pub(crate) fn with_action(mut self, action: MovementAction) -> Self {
        self.control_flags |= action.control_flag();
        self
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Default)]
pub(crate) struct HeldControls {
    pressed: u16,
    flying: bool,
    always_run: bool,
    stop_pending: bool,
    stop_sequence: u64,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) struct BodyRotation {
    seed: Option<u64>,
    current: Quat,
    updated_at: Option<Instant>,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl Default for BodyRotation {
    fn default() -> Self {
        Self { seed: None, current: Quat::IDENTITY, updated_at: None }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl KeyBinding {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn from_key(key: &Key) -> Option<Self> {
        match key {
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("w") => Some(Self::Movement(MovementKey::Forward)),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("s") => Some(Self::Movement(MovementKey::Backward)),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("a") => Some(Self::Movement(MovementKey::TurnLeft)),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("d") => Some(Self::Movement(MovementKey::TurnRight)),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("e") => Some(Self::Movement(MovementKey::Jump)),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("c") => Some(Self::Movement(MovementKey::Down)),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("f") => Some(Self::ToggleFlying),
            Key::Character(character) if character.as_str().eq_ignore_ascii_case("r") => Some(Self::ToggleRun),
            Key::Named(NamedKey::ArrowUp) => Some(Self::Movement(MovementKey::ForwardArrow)),
            Key::Named(NamedKey::ArrowDown) => Some(Self::Movement(MovementKey::BackwardArrow)),
            Key::Named(NamedKey::ArrowLeft) => Some(Self::Movement(MovementKey::TurnLeftArrow)),
            Key::Named(NamedKey::ArrowRight) => Some(Self::Movement(MovementKey::TurnRightArrow)),
            Key::Named(NamedKey::PageUp) => Some(Self::Movement(MovementKey::JumpPage)),
            Key::Named(NamedKey::PageDown) => Some(Self::Movement(MovementKey::DownPage)),
            Key::Named(NamedKey::Home) => Some(Self::ToggleFlying),
            Key::Named(NamedKey::Space) => Some(Self::Stop),
            _ => None,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl MovementKey {
    fn mask(self) -> u16 {
        1 << (self as u8)
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl HeldControls {
    pub(crate) fn set_pressed(&mut self, key: MovementKey, pressed: bool) -> bool {
        let mask = key.mask();
        let previous = self.pressed;
        if pressed {
            self.pressed |= mask;
        } else {
            self.pressed &= !mask;
        }
        self.pressed != previous
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn keyboard_event(&mut self, key: &Key, state: ElementState, repeat: bool, captured: bool) -> Option<(KeyBinding, bool)> {
        let binding = KeyBinding::from_key(key)?;
        if state == ElementState::Released {
            let changed = match binding {
                KeyBinding::Movement(key) => self.set_pressed(key, false),
                _ => false,
            };
            return Some((binding, changed));
        }
        if repeat {
            return Some((binding, false));
        }
        if captured {
            return None;
        }
        let changed = match binding {
            KeyBinding::Movement(key) => self.set_pressed(key, true),
            KeyBinding::ToggleFlying => {
                self.toggle_flying();
                true
            }
            KeyBinding::ToggleRun => {
                self.toggle_run();
                true
            }
            KeyBinding::Stop => {
                self.stop();
                true
            }
        };
        Some((binding, changed))
    }

    pub(crate) fn toggle_flying(&mut self) {
        self.flying = !self.flying;
    }

    pub(crate) fn toggle_run(&mut self) {
        self.always_run = !self.always_run;
    }

    pub(crate) fn stop(&mut self) {
        self.pressed = 0;
        self.request_stop();
    }

    pub(crate) fn release_all(&mut self) {
        self.pressed = 0;
        self.request_stop();
    }

    pub(crate) fn release_and_observe(&mut self, body_rotation: Quat) -> MovementObservation {
        self.release_all();
        self.observation(false, body_rotation)
    }

    pub(crate) fn active(&self) -> bool {
        self.pressed != 0
    }

    pub(crate) fn yaw(&self, shift: bool) -> f32 {
        if shift {
            return 0.0;
        }
        let left = self.is_pressed(MovementKey::TurnLeft) || self.is_pressed(MovementKey::TurnLeftArrow);
        let right = self.is_pressed(MovementKey::TurnRight) || self.is_pressed(MovementKey::TurnRightArrow);
        f32::from(left as u8) - f32::from(right as u8)
    }

    pub(crate) fn observation(&mut self, shift: bool, body_rotation: Quat) -> MovementObservation {
        let mut control_flags = 0;
        let forward = self.is_pressed(MovementKey::Forward) || self.is_pressed(MovementKey::ForwardArrow);
        let backward = self.is_pressed(MovementKey::Backward) || self.is_pressed(MovementKey::BackwardArrow);
        let left = self.is_pressed(MovementKey::TurnLeft) || self.is_pressed(MovementKey::TurnLeftArrow);
        let right = self.is_pressed(MovementKey::TurnRight) || self.is_pressed(MovementKey::TurnRightArrow);
        if forward {
            control_flags |= AT_POS | FAST_AT;
        }
        if backward {
            control_flags |= AT_NEG | FAST_AT;
        }
        if shift {
            if left {
                control_flags |= LEFT_POS | FAST_LEFT;
            }
            if right {
                control_flags |= LEFT_NEG | FAST_LEFT;
            }
        } else {
            if left {
                control_flags |= YAW_POS;
            }
            if right {
                control_flags |= YAW_NEG;
            }
        }
        if self.is_pressed(MovementKey::Jump) || self.is_pressed(MovementKey::JumpPage) {
            control_flags |= UP_POS | FAST_UP;
        }
        if self.is_pressed(MovementKey::Down) || self.is_pressed(MovementKey::DownPage) {
            control_flags |= UP_NEG | FAST_UP;
        }
        if self.flying {
            control_flags |= FLY;
        }
        if self.stop_pending {
            control_flags |= STOP;
            self.stop_pending = false;
        }
        MovementObservation { control_flags, body_rotation, always_run: self.always_run, stop_sequence: self.stop_sequence }
    }

    fn is_pressed(&self, key: MovementKey) -> bool {
        self.pressed & key.mask() != 0
    }

    fn request_stop(&mut self) {
        self.stop_pending = true;
        self.stop_sequence = self.stop_sequence.wrapping_add(1);
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl BodyRotation {
    pub(crate) fn current(&self) -> Quat {
        self.current
    }

    pub(crate) fn update(&mut self, seed: Option<(u64, Quat)>, yaw: f32, now: Instant) -> Quat {
        if let Some((generation, rotation)) = seed
            && self.seed != Some(generation)
            && let Some(rotation) = yaw_rotation(rotation)
        {
            self.seed = Some(generation);
            self.current = rotation;
        }
        let dt = self.updated_at.map_or(Duration::ZERO, |previous| now.saturating_duration_since(previous).min(MAX_TURN_STEP));
        self.updated_at = Some(now);
        if yaw != 0.0 {
            self.current = (Quat::from_rotation_z(yaw * TURN_RATE * dt.as_secs_f32()) * self.current).normalize();
        }
        self.current
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn yaw_rotation(rotation: Quat) -> Option<Quat> {
    let forward = rotation * Vec3::X;
    let forward = Vec3::new(forward.x, forward.y, 0.0).try_normalize()?;
    Some(Quat::from_rotation_z(forward.y.atan2(forward.x)))
}

#[cfg(all(test, any(target_os = "windows", target_os = "linux")))]
mod tests {
    use std::time::{Duration, Instant};

    use math::{Quat, Vec3};
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    use winit::{event::ElementState, keyboard::Key};

    use super::{AT_NEG, AT_POS, FAST_AT, FAST_LEFT, FAST_UP, FLY, LEFT_NEG, LEFT_POS, STOP, UP_NEG, UP_POS, YAW_NEG, YAW_POS};
    use super::{BodyRotation, HeldControls, KeyBinding, MovementKey};

    #[test]
    fn keyboard_bindings_derive_official_control_flags() {
        let mut controls = HeldControls::default();
        for (key, expected) in [
            ("w", AT_POS | FAST_AT),
            ("s", AT_NEG | FAST_AT),
            ("a", YAW_POS),
            ("d", YAW_NEG),
            ("e", UP_POS | FAST_UP),
            ("c", UP_NEG | FAST_UP),
        ] {
            let binding = KeyBinding::from_key(&Key::Character(key.into())).expect("movement key");
            let KeyBinding::Movement(key) = binding else { panic!("movement binding") };
            controls.set_pressed(key, true);
            assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, expected);
            controls.set_pressed(key, false);
        }
        controls.set_pressed(MovementKey::TurnLeft, true);
        assert_eq!(controls.observation(true, Quat::IDENTITY).control_flags, LEFT_POS | FAST_LEFT);
        controls.set_pressed(MovementKey::TurnLeft, false);
        controls.set_pressed(MovementKey::TurnRightArrow, true);
        assert_eq!(controls.observation(true, Quat::IDENTITY).control_flags, LEFT_NEG | FAST_LEFT);
        controls.set_pressed(MovementKey::TurnRightArrow, false);
        controls.set_pressed(MovementKey::Forward, true);
        controls.set_pressed(MovementKey::BackwardArrow, true);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, AT_POS | AT_NEG | FAST_AT);
    }

    #[test]
    fn v_is_not_a_camera_mode_toggle() {
        assert_eq!(KeyBinding::from_key(&Key::Character("v".into())), None);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn captured_press_does_not_move() {
        let mut controls = HeldControls::default();
        let key = Key::Character("w".into());
        assert_eq!(controls.keyboard_event(&key, ElementState::Pressed, false, true), None);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, 0);

        controls.keyboard_event(&key, ElementState::Pressed, false, false).expect("forward press");
        controls.keyboard_event(&key, ElementState::Released, false, true).expect("forward release");
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, 0);
    }

    #[test]
    fn toggles_and_focus_loss_publish_fly_run_and_stop_state() {
        let mut controls = HeldControls::default();
        controls.set_pressed(MovementKey::Forward, true);
        controls.toggle_flying();
        controls.toggle_run();
        let movement = controls.observation(false, Quat::IDENTITY);
        assert_eq!(movement.control_flags, AT_POS | FAST_AT | FLY);
        assert!(movement.always_run);

        let stopped = controls.release_and_observe(Quat::IDENTITY);
        assert_eq!(stopped.control_flags, STOP | FLY);
        assert!(stopped.stop_sequence > movement.stop_sequence);
        assert!(stopped.always_run);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, FLY);
    }

    #[test]
    fn body_rotation_uses_ninety_degree_per_second_yaw_rate() {
        let start = Instant::now();
        let mut rotation = BodyRotation::default();
        assert_eq!(rotation.update(Some((1, Quat::IDENTITY)), 1.0, start), Quat::IDENTITY);
        let mut current = Quat::IDENTITY;
        for step in 1..=10 {
            current = rotation.update(Some((1, Quat::IDENTITY)), 1.0, start + Duration::from_millis(step * 100));
        }
        assert!((current * Vec3::X).distance(Vec3::Y) < 1e-5);
    }

    #[test]
    fn simulator_rotation_is_ignored_after_seed() {
        let start = Instant::now();
        let mut rotation = BodyRotation::default();
        rotation.update(Some((3, Quat::IDENTITY)), 1.0, start);
        let turned = rotation.update(Some((3, Quat::IDENTITY)), 1.0, start + Duration::from_millis(100));
        let current = rotation.update(Some((3, Quat::from_rotation_z(0.2))), 0.0, start + Duration::from_millis(200));
        assert!((current * Vec3::X).distance(turned * Vec3::X) < 1e-5);
    }

    #[test]
    fn arrival_marker_reseeds_rotation() {
        let start = Instant::now();
        let mut rotation = BodyRotation::default();
        rotation.update(Some((1, Quat::IDENTITY)), 1.0, start);
        rotation.update(Some((1, Quat::IDENTITY)), 1.0, start + Duration::from_millis(100));

        let arrival = Quat::from_rotation_z(0.7);
        let current = rotation.update(Some((2, arrival)), 0.0, start + Duration::from_millis(200));
        assert!((current * Vec3::X).distance(arrival * Vec3::X) < 1e-5);
    }

    #[test]
    fn seed_rotation_is_flattened() {
        let start = Instant::now();
        let mut rotation = BodyRotation::default();
        let seed = Quat::from_rotation_y(0.8) * Quat::from_rotation_z(0.5);
        let current = rotation.update(Some((1, seed)), 0.0, start);
        let forward = seed * Vec3::X;
        let expected = Vec3::new(forward.x, forward.y, 0.0).normalize();
        assert!((current * Vec3::X - expected).length() < 1e-5);
    }
}
