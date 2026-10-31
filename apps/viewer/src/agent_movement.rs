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
pub(crate) const FLY: u32 = 0x0000_2000;
pub(crate) const STOP: u32 = 0x0000_4000;
const SIT_ON_GROUND: u32 = 0x0002_0000;
const STAND_UP: u32 = 0x0001_0000;
const NUDGE_AT_POS: u32 = 0x0008_0000;
const NUDGE_AT_NEG: u32 = 0x0010_0000;
const NUDGE_LEFT_POS: u32 = 0x0020_0000;
const NUDGE_LEFT_NEG: u32 = 0x0040_0000;
pub(crate) const BODY_ROTATION_DOT_THRESHOLD: f32 = 0.9997;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const TURN_RATE: f32 = std::f32::consts::FRAC_PI_2;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const MAX_TURN_STEP: Duration = Duration::from_millis(100);
#[cfg(any(target_os = "windows", target_os = "linux"))]
const SITTING_NUDGE_TIME: Duration = Duration::from_millis(250);
#[cfg(any(target_os = "windows", target_os = "linux"))]
const SITTING_NUDGE_RATE: f32 = 0.05;

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

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SittingRunMode {
    Forward,
    Backward,
    SlideLeft,
    SlideRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MovementObservation {
    pub(crate) control_flags: u32,
    pub(crate) body_rotation: Quat,
    pub(crate) always_run: bool,
    pub(crate) stop_sequence: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScriptControlGrabs {
    direct: [u32; 32],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SittingCameraInput {
    pub(crate) around: f32,
    pub(crate) over: f32,
    pub(crate) distance: f32,
    pub(crate) pan_horizontal: f32,
    pub(crate) pan_vertical: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SittingActions {
    pub(crate) control_flags: u32,
    pub(crate) camera: SittingCameraInput,
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

impl ScriptControlGrabs {
    pub(crate) fn apply(&mut self, take: bool, controls: u32, pass_to_agent: bool) -> bool {
        if pass_to_agent {
            return false;
        }
        let before = *self;
        for (index, count) in self.direct.iter_mut().enumerate() {
            if controls & (1 << index) == 0 {
                continue;
            }
            if take {
                *count = count.saturating_add(1);
            } else {
                *count = count.saturating_sub(1);
            }
        }
        *self != before
    }

    pub(crate) fn clear(&mut self) -> bool {
        let changed = self.direct.iter().any(|count| *count != 0);
        *self = Self::default();
        changed
    }

    fn contains(self, controls: u32) -> bool {
        self.direct.iter().enumerate().any(|(index, count)| *count > 0 && controls & (1 << index) != 0)
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Default)]
pub(crate) struct HeldControls {
    pressed: u16,
    pressed_at: [Option<Instant>; 12],
    processed_run_press: [Option<Instant>; 12],
    last_run_tap: Option<(SittingRunMode, Instant)>,
    flying: bool,
    always_run: bool,
    temporary_running: bool,
    temporary_run_mode: Option<SittingRunMode>,
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

    fn sitting_run_mode(self) -> Option<SittingRunMode> {
        match self {
            Self::Forward | Self::ForwardArrow => Some(SittingRunMode::Forward),
            Self::Backward | Self::BackwardArrow => Some(SittingRunMode::Backward),
            Self::TurnLeft | Self::TurnLeftArrow => Some(SittingRunMode::SlideLeft),
            Self::TurnRight | Self::TurnRightArrow => Some(SittingRunMode::SlideRight),
            Self::Jump | Self::JumpPage | Self::Down | Self::DownPage => None,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl HeldControls {
    #[cfg(test)]
    pub(crate) fn set_pressed(&mut self, key: MovementKey, pressed: bool) -> bool {
        self.set_pressed_at(key, pressed, Instant::now())
    }

    fn set_pressed_at(&mut self, key: MovementKey, pressed: bool, now: Instant) -> bool {
        let mask = key.mask();
        let previous = self.pressed;
        if pressed {
            self.pressed |= mask;
            if previous & mask == 0 {
                self.pressed_at[key as usize] = Some(now);
            }
        } else {
            self.pressed &= !mask;
            self.pressed_at[key as usize] = None;
        }
        self.pressed != previous
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn keyboard_event(
        &mut self,
        key: &Key,
        state: ElementState,
        repeat: bool,
        captured: bool,
        seated: bool,
    ) -> Option<(KeyBinding, bool)> {
        self.keyboard_event_at(key, state, repeat, captured, seated, Instant::now())
    }

    fn keyboard_event_at(
        &mut self,
        key: &Key,
        state: ElementState,
        repeat: bool,
        captured: bool,
        seated: bool,
        now: Instant,
    ) -> Option<(KeyBinding, bool)> {
        let binding = KeyBinding::from_key(key)?;
        if state == ElementState::Released {
            let changed = match binding {
                KeyBinding::Movement(key) => {
                    let changed = self.set_pressed_at(key, false, now);
                    if !self.always_run && key.sitting_run_mode().is_some_and(|mode| self.temporary_run_mode == Some(mode)) {
                        self.temporary_running = false;
                        self.temporary_run_mode = None;
                    }
                    changed
                }
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
            KeyBinding::Movement(key) => self.set_pressed_at(key, true, now),
            KeyBinding::ToggleFlying => self.toggle_flying(seated),
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

    pub(crate) fn toggle_flying(&mut self, seated: bool) -> bool {
        if seated && !self.flying {
            return false;
        }
        self.flying = !self.flying;
        true
    }

    pub(crate) fn clear_flying(&mut self) {
        self.flying = false;
    }

    pub(crate) fn toggle_run(&mut self) {
        self.always_run = !self.always_run;
        if !self.always_run {
            self.temporary_running = false;
            self.temporary_run_mode = None;
        }
    }

    pub(crate) fn stop(&mut self) {
        self.pressed = 0;
        self.pressed_at = [None; 12];
        self.reset_sitting_run();
        self.request_stop();
    }

    pub(crate) fn release_all(&mut self) {
        self.pressed = 0;
        self.pressed_at = [None; 12];
        self.reset_sitting_run();
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
        MovementObservation { control_flags, body_rotation, always_run: self.is_running(), stop_sequence: self.stop_sequence }
    }

    pub(crate) fn sitting_actions(
        &mut self,
        shift: bool,
        alt: bool,
        control: bool,
        sit_camera_enabled: bool,
        grabs: ScriptControlGrabs,
        now: Instant,
    ) -> SittingActions {
        let mut actions = SittingActions::default();
        let left = self.is_pressed(MovementKey::TurnLeft) || self.is_pressed(MovementKey::TurnLeftArrow);
        let right = self.is_pressed(MovementKey::TurnRight) || self.is_pressed(MovementKey::TurnRightArrow);
        let forward = self.is_pressed(MovementKey::Forward) || self.is_pressed(MovementKey::ForwardArrow);
        let backward = self.is_pressed(MovementKey::Backward) || self.is_pressed(MovementKey::BackwardArrow);
        let up = self.is_pressed(MovementKey::Jump) || self.is_pressed(MovementKey::JumpPage);
        let down = self.is_pressed(MovementKey::Down) || self.is_pressed(MovementKey::DownPage);
        if control && alt && shift {
            actions.camera.pan_horizontal += self.key_rate(MovementKey::TurnLeft, MovementKey::TurnLeftArrow, now);
            actions.camera.pan_horizontal -= self.key_rate(MovementKey::TurnRight, MovementKey::TurnRightArrow, now);
            actions.camera.pan_vertical += self.key_rate(MovementKey::Forward, MovementKey::ForwardArrow, now);
            actions.camera.pan_vertical -= self.key_rate(MovementKey::Backward, MovementKey::BackwardArrow, now);
            return actions;
        }
        if control && alt && !shift {
            actions.camera.over += self.key_rate(MovementKey::Jump, MovementKey::JumpPage, now).max(self.key_rate(
                MovementKey::Forward,
                MovementKey::ForwardArrow,
                now,
            ));
            actions.camera.over -= self.key_rate(MovementKey::Down, MovementKey::DownPage, now).max(self.key_rate(
                MovementKey::Backward,
                MovementKey::BackwardArrow,
                now,
            ));
            return actions;
        }
        if alt && !shift && !control {
            actions.camera.around += self.key_rate(MovementKey::TurnRight, MovementKey::TurnRightArrow, now);
            actions.camera.around -= self.key_rate(MovementKey::TurnLeft, MovementKey::TurnLeftArrow, now);
            actions.camera.distance += self.key_rate(MovementKey::Forward, MovementKey::ForwardArrow, now);
            actions.camera.distance -= self.key_rate(MovementKey::Backward, MovementKey::BackwardArrow, now);
            if sit_camera_enabled || grabs.contains(UP_POS) {
                if up {
                    actions.control_flags |= UP_POS | FAST_UP;
                }
            } else {
                actions.camera.over += self.key_rate(MovementKey::Jump, MovementKey::JumpPage, now);
            }
            if sit_camera_enabled || grabs.contains(UP_NEG) {
                if down {
                    actions.control_flags |= UP_NEG | FAST_UP;
                }
            } else {
                actions.camera.over -= self.key_rate(MovementKey::Down, MovementKey::DownPage, now);
            }
            return actions;
        }
        if alt || control {
            return actions;
        }
        if shift {
            if left {
                self.note_sitting_run_press(MovementKey::TurnLeft);
                actions.control_flags |= if self.key_is_nudging(MovementKey::TurnLeft, MovementKey::TurnLeftArrow, now) {
                    NUDGE_LEFT_POS
                } else {
                    LEFT_POS | FAST_LEFT
                };
            }
            if right {
                self.note_sitting_run_press(MovementKey::TurnRight);
                actions.control_flags |= if self.key_is_nudging(MovementKey::TurnRight, MovementKey::TurnRightArrow, now) {
                    NUDGE_LEFT_NEG
                } else {
                    LEFT_NEG | FAST_LEFT
                };
            }
        } else if sit_camera_enabled || grabs.contains(YAW_POS | YAW_NEG) || self.is_running() {
            if left {
                self.note_sitting_run_press(MovementKey::TurnLeft);
                actions.control_flags |= YAW_POS;
            }
            if right {
                self.note_sitting_run_press(MovementKey::TurnRight);
                actions.control_flags |= YAW_NEG;
            }
        } else {
            actions.camera.around += self.key_rate(MovementKey::TurnRight, MovementKey::TurnRightArrow, now);
            actions.camera.around -= self.key_rate(MovementKey::TurnLeft, MovementKey::TurnLeftArrow, now);
        }

        if sit_camera_enabled || grabs.contains(AT_POS) || (self.temporary_running && !self.always_run) {
            if forward {
                self.note_sitting_run_press(MovementKey::Forward);
                actions.control_flags |=
                    if self.key_is_nudging(MovementKey::Forward, MovementKey::ForwardArrow, now) { NUDGE_AT_POS } else { AT_POS | FAST_AT };
            }
        } else {
            actions.camera.distance += self.key_rate(MovementKey::Forward, MovementKey::ForwardArrow, now);
        }
        if sit_camera_enabled || grabs.contains(AT_NEG) || (self.temporary_running && !self.always_run) {
            if backward {
                self.note_sitting_run_press(MovementKey::Backward);
                actions.control_flags |= if self.key_is_nudging(MovementKey::Backward, MovementKey::BackwardArrow, now) {
                    NUDGE_AT_NEG
                } else {
                    AT_NEG | FAST_AT
                };
            }
        } else {
            actions.camera.distance -= self.key_rate(MovementKey::Backward, MovementKey::BackwardArrow, now);
        }

        if sit_camera_enabled || grabs.contains(UP_POS) {
            if up {
                actions.control_flags |= UP_POS | FAST_UP;
            }
        } else {
            actions.camera.over += self.key_rate(MovementKey::Jump, MovementKey::JumpPage, now);
        }
        if sit_camera_enabled || grabs.contains(UP_NEG) {
            if down {
                actions.control_flags |= UP_NEG | FAST_UP;
            }
        } else {
            actions.camera.over -= self.key_rate(MovementKey::Down, MovementKey::DownPage, now);
        }

        actions
    }

    pub(crate) fn sitting_observation(&mut self, body_rotation: Quat, actions: SittingActions) -> MovementObservation {
        let mut observation = self.observation(false, body_rotation);
        observation.control_flags &= FLY | STOP;
        observation.control_flags |= actions.control_flags;
        observation.always_run = self.is_running();
        observation
    }

    fn is_running(&self) -> bool {
        self.always_run || self.temporary_running
    }

    fn note_sitting_run_press(&mut self, key: MovementKey) {
        let Some(pressed_at) = self.pressed_at[key as usize] else { return };
        if self.processed_run_press[key as usize] == Some(pressed_at) {
            return;
        }
        self.processed_run_press[key as usize] = Some(pressed_at);
        if self.is_running() {
            return;
        }
        let Some(mode) = key.sitting_run_mode() else { return };
        let running = self
            .last_run_tap
            .is_some_and(|(last_mode, last_at)| last_mode == mode && pressed_at.saturating_duration_since(last_at) < SITTING_NUDGE_TIME);
        self.last_run_tap = Some((mode, pressed_at));
        if running {
            self.temporary_running = true;
            self.temporary_run_mode = Some(mode);
        }
    }

    fn reset_sitting_run(&mut self) {
        self.processed_run_press = [None; 12];
        self.last_run_tap = None;
        self.temporary_running = false;
        self.temporary_run_mode = None;
    }

    fn key_rate(&self, first: MovementKey, second: MovementKey, now: Instant) -> f32 {
        [first, second]
            .into_iter()
            .filter(|key| self.is_pressed(*key))
            .filter_map(|key| self.pressed_at[key as usize])
            .map(|pressed_at| {
                let elapsed = now.saturating_duration_since(pressed_at);
                if elapsed >= SITTING_NUDGE_TIME {
                    1.0
                } else {
                    SITTING_NUDGE_RATE + elapsed.as_secs_f32() * (1.0 - SITTING_NUDGE_RATE) / SITTING_NUDGE_TIME.as_secs_f32()
                }
            })
            .fold(0.0, f32::max)
    }

    fn key_is_nudging(&self, first: MovementKey, second: MovementKey, now: Instant) -> bool {
        [first, second].into_iter().any(|key| {
            self.is_pressed(key)
                && self.pressed_at[key as usize].is_some_and(|pressed_at| now.saturating_duration_since(pressed_at) < SITTING_NUDGE_TIME)
        })
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

    pub(crate) fn reset_axes(&mut self, rotation: Quat) {
        if let Some(rotation) = yaw_rotation(rotation) {
            self.current = rotation;
        }
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

    use super::{
        AT_NEG, AT_POS, FAST_AT, FAST_LEFT, FAST_UP, FLY, LEFT_NEG, LEFT_POS, NUDGE_AT_POS, NUDGE_LEFT_POS, SITTING_NUDGE_RATE,
        SITTING_NUDGE_TIME, STAND_UP, STOP, UP_NEG, UP_POS, YAW_NEG, YAW_POS,
    };
    use super::{BodyRotation, HeldControls, KeyBinding, MovementKey, ScriptControlGrabs};

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
        assert_eq!(controls.keyboard_event(&key, ElementState::Pressed, false, true, false), None);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, 0);

        controls.keyboard_event(&key, ElementState::Pressed, false, false, false).expect("forward press");
        controls.keyboard_event(&key, ElementState::Released, false, true, false).expect("forward release");
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, 0);
    }

    #[test]
    fn toggles_and_focus_loss_publish_fly_run_and_stop_state() {
        let mut controls = HeldControls::default();
        controls.set_pressed(MovementKey::Forward, true);
        controls.toggle_flying(false);
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
    fn seated_agent_cannot_start_flying() {
        let mut controls = HeldControls::default();
        controls.toggle_flying(true);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags & FLY, 0);

        controls.toggle_flying(false);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags & FLY, FLY);

        controls.toggle_flying(true);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags & FLY, 0);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn seated_movement_keys_do_not_stand() {
        let mut controls = HeldControls::default();
        for (key, expected) in [("w", AT_POS | FAST_AT), ("e", UP_POS | FAST_UP)] {
            let key = Key::Character(key.into());
            controls.keyboard_event(&key, ElementState::Pressed, false, false, true).expect("seated movement press");
            let flags = controls.observation(false, Quat::IDENTITY).control_flags;
            assert_eq!(flags, expected);
            assert_eq!(flags & STAND_UP, 0);
            controls.keyboard_event(&key, ElementState::Released, false, false, true).expect("seated movement release");
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn script_grabs_forward_controls_and_release_restores_orbit() {
        let start = Instant::now();
        let mut controls = HeldControls::default();
        controls.keyboard_event(&Key::Character("w".into()), ElementState::Pressed, false, false, true).expect("forward press");
        let mut grabs = ScriptControlGrabs::default();
        assert!(grabs.apply(true, AT_POS, false));
        assert!(!grabs.apply(true, AT_POS, true));

        let forwarded = controls.sitting_actions(false, false, false, false, grabs, start + Duration::from_secs(1));
        assert_eq!(forwarded.control_flags, AT_POS | FAST_AT);
        assert_eq!(forwarded.camera.distance, 0.0);

        assert!(grabs.apply(false, AT_POS, false));
        let orbited = controls.sitting_actions(false, false, false, false, grabs, start + Duration::from_secs(2));
        assert_eq!(orbited.control_flags, 0);
        assert_eq!(orbited.camera.distance, 1.0);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn sitting_orbit_ramps_with_viewer_nudge_rate() {
        let start = Instant::now();
        let key = Key::Character("a".into());
        let mut controls = HeldControls::default();
        controls.keyboard_event_at(&key, ElementState::Pressed, false, false, true, start).expect("left press");

        for (elapsed, expected) in [(Duration::ZERO, -SITTING_NUDGE_RATE), (Duration::from_millis(125), -0.525), (SITTING_NUDGE_TIME, -1.0)]
        {
            let actions = controls.sitting_actions(false, false, false, false, ScriptControlGrabs::default(), start + elapsed);
            assert!((actions.camera.around - expected).abs() < 1e-6);
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn sitting_modifier_keys_match_viewer_bindings() {
        let start = Instant::now();
        let mut controls = HeldControls::default();
        for key in ["a", "w", "e"] {
            controls.keyboard_event_at(&Key::Character(key.into()), ElementState::Pressed, false, false, true, start).expect("sitting key");
        }

        let alt = controls.sitting_actions(false, true, false, false, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
        assert_eq!(alt.camera.around, -1.0);
        assert_eq!(alt.camera.over, 1.0);
        assert_eq!(alt.camera.distance, 1.0);
        assert_eq!(alt.control_flags, 0);

        let control_alt = controls.sitting_actions(false, true, true, false, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
        assert_eq!(control_alt.camera.around, 0.0);
        assert_eq!(control_alt.camera.over, 1.0);
        assert_eq!(control_alt.camera.distance, 0.0);
        assert_eq!(control_alt.control_flags, 0);

        let shift = controls.sitting_actions(true, false, false, false, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
        assert_eq!(shift.control_flags, LEFT_POS | FAST_LEFT);
        assert_eq!(shift.camera.around, 0.0);
        assert_eq!(shift.camera.over, 1.0);
        assert_eq!(shift.camera.distance, 1.0);

        let control_alt_shift =
            controls.sitting_actions(true, true, true, false, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
        assert_eq!(control_alt_shift.control_flags, 0);
        assert_eq!(control_alt_shift.camera.pan_horizontal, 1.0);
        assert_eq!(control_alt_shift.camera.pan_vertical, 1.0);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn sitting_alt_vertical_respects_camera_and_grabs() {
        let start = Instant::now();
        let mut controls = HeldControls::default();
        for key in ["e", "c"] {
            controls
                .keyboard_event_at(&Key::Character(key.into()), ElementState::Pressed, false, false, true, start)
                .expect("vertical key");
        }

        for (key, expected) in [("e", 1.0), ("c", -1.0)] {
            let mut orbiting = HeldControls::default();
            orbiting
                .keyboard_event_at(&Key::Character(key.into()), ElementState::Pressed, false, false, true, start)
                .expect("vertical key");
            let orbit = orbiting.sitting_actions(false, true, false, false, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
            assert_eq!(orbit.control_flags, 0);
            assert_eq!(orbit.camera.over, expected);
        }

        let forwarded = controls.sitting_actions(false, true, false, true, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
        assert_eq!(forwarded.control_flags, UP_POS | UP_NEG | FAST_UP);
        assert_eq!(forwarded.camera, super::SittingCameraInput::default());

        let mut grabs = ScriptControlGrabs::default();
        assert!(grabs.apply(true, UP_POS, false));
        let partially_forwarded = controls.sitting_actions(false, true, false, false, grabs, start + SITTING_NUDGE_TIME);
        assert_eq!(partially_forwarded.control_flags, UP_POS | FAST_UP);
        assert_eq!(partially_forwarded.camera.over, -1.0);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn always_run_forwards_turns_not_steps() {
        let start = Instant::now();
        let mut controls = HeldControls::default();
        controls.toggle_run();
        for key in ["a", "w"] {
            controls.keyboard_event_at(&Key::Character(key.into()), ElementState::Pressed, false, false, true, start).expect("sitting key");
        }

        let actions = controls.sitting_actions(false, false, false, false, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME);
        assert_eq!(actions.control_flags, YAW_POS);
        assert_eq!(actions.camera.around, 0.0);
        assert_eq!(actions.camera.over, 0.0);
        assert_eq!(actions.camera.distance, 1.0);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn forwarded_sitting_movement_uses_viewer_nudge_then_normal_flags() {
        let start = Instant::now();
        let mut controls = HeldControls::default();
        controls.keyboard_event_at(&Key::Character("w".into()), ElementState::Pressed, false, false, true, start).expect("forward press");

        assert_eq!(controls.sitting_actions(false, false, false, true, ScriptControlGrabs::default(), start).control_flags, NUDGE_AT_POS);
        assert_eq!(
            controls.sitting_actions(false, false, false, true, ScriptControlGrabs::default(), start + SITTING_NUDGE_TIME).control_flags,
            AT_POS | FAST_AT
        );

        let mut sliding = HeldControls::default();
        sliding.keyboard_event_at(&Key::Character("a".into()), ElementState::Pressed, false, false, true, start).expect("slide press");
        assert_eq!(sliding.sitting_actions(true, false, false, false, ScriptControlGrabs::default(), start).control_flags, NUDGE_LEFT_POS);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn sitting_double_tap_runs_until_release() {
        let start = Instant::now();
        let forward = Key::Character("w".into());
        let mut controls = HeldControls::default();
        let mut grabs = ScriptControlGrabs::default();
        assert!(grabs.apply(true, AT_POS, false));

        controls.keyboard_event_at(&forward, ElementState::Pressed, false, false, true, start).expect("first forward press");
        let first = controls.sitting_actions(false, false, false, false, grabs, start);
        assert_eq!(first.control_flags, NUDGE_AT_POS);
        controls
            .keyboard_event_at(&forward, ElementState::Released, false, false, true, start + Duration::from_millis(50))
            .expect("first forward release");

        controls
            .keyboard_event_at(&forward, ElementState::Pressed, false, false, true, start + Duration::from_millis(150))
            .expect("second forward press");
        let second = controls.sitting_actions(false, false, false, false, grabs, start + Duration::from_millis(150));
        assert_eq!(second.control_flags, NUDGE_AT_POS);
        assert!(controls.sitting_observation(Quat::IDENTITY, second).always_run);

        let without_grab =
            controls.sitting_actions(false, false, false, false, ScriptControlGrabs::default(), start + Duration::from_millis(175));
        assert_eq!(without_grab.control_flags, NUDGE_AT_POS);
        assert_eq!(without_grab.camera.distance, 0.0);

        controls
            .keyboard_event_at(&forward, ElementState::Released, false, false, true, start + Duration::from_millis(200))
            .expect("second forward release");
        let stopped =
            controls.sitting_actions(false, false, false, false, ScriptControlGrabs::default(), start + Duration::from_millis(200));
        assert!(!controls.sitting_observation(Quat::IDENTITY, stopped).always_run);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn releasing_controls_clears_sitting_camera_input() {
        let start = Instant::now();
        let mut controls = HeldControls::default();
        controls.keyboard_event_at(&Key::Character("w".into()), ElementState::Pressed, false, false, true, start).expect("forward press");
        controls.release_all();
        assert_eq!(
            controls.sitting_actions(false, false, false, false, ScriptControlGrabs::default(), start + Duration::from_secs(1)),
            super::SittingActions::default()
        );
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn sitting_clears_flying() {
        let mut controls = HeldControls::default();
        controls.toggle_flying(false);
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags & FLY, FLY);

        controls.clear_flying();
        assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags & FLY, 0);
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
