#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use super::{
    AT_NEG, AT_POS, FAST_AT, FAST_LEFT, FAST_UP, FLY, LEFT_NEG, LEFT_POS, MovementObservation, STOP, UP_NEG, UP_POS, YAW_NEG, YAW_POS,
};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use super::{HeldControls, MovementKey};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use math::Quat;

pub(super) const NUDGE_AT_POS: u32 = 0x0008_0000;
const NUDGE_AT_NEG: u32 = 0x0010_0000;
pub(super) const NUDGE_LEFT_POS: u32 = 0x0020_0000;
const NUDGE_LEFT_NEG: u32 = 0x0040_0000;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(super) const SITTING_NUDGE_TIME: Duration = Duration::from_millis(250);
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(super) const SITTING_NUDGE_RATE: f32 = 0.05;
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SittingRunMode {
    Forward,
    Backward,
    SlideLeft,
    SlideRight,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl MovementKey {
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
                actions.control_flags |= YAW_POS;
            }
            if right {
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

    pub(super) fn release_sitting_key(&mut self, key: MovementKey, now: Instant) -> bool {
        let changed = self.set_pressed_at(key, false, now);
        if !self.always_run && key.sitting_run_mode().is_some_and(|mode| self.temporary_run_mode == Some(mode)) {
            self.temporary_running = false;
            self.temporary_run_mode = None;
        }
        changed
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

    pub(super) fn reset_sitting_run(&mut self) {
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
}
