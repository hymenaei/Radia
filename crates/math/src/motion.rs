use crate::{DVec3, Vec3};

pub const PHYSICS_TIMESTEP: f64 = 1.0 / 45.0;
pub const PHASE_OUT_TIME: f64 = 2.0;
pub const MAX_INTERPOLATION_TIME: f64 = 3.0;

pub fn effective_elapsed(elapsed: f64, time_dilation: f64, phase_out: bool) -> f64 {
    let elapsed = if elapsed.is_finite() { elapsed.max(0.0) } else { 0.0 };
    let time_dilation = if time_dilation.is_finite() { time_dilation.clamp(0.0, 1.0) } else { 0.0 };
    let phase_elapsed = if !phase_out || elapsed <= PHASE_OUT_TIME {
        elapsed
    } else if elapsed < MAX_INTERPOLATION_TIME {
        let phase = elapsed - PHASE_OUT_TIME;
        PHASE_OUT_TIME + phase - 0.5 * phase * phase
    } else {
        PHASE_OUT_TIME + 0.5
    };
    phase_elapsed * time_dilation
}

pub fn position_at_effective(start: DVec3, velocity: Vec3, acceleration: Vec3, elapsed: f64) -> DVec3 {
    start + velocity.as_dvec3() * elapsed + acceleration.as_dvec3() * (0.5 * elapsed * (elapsed - PHYSICS_TIMESTEP))
}

pub fn linear_position(start: DVec3, velocity: Vec3, acceleration: Vec3, elapsed: f64, time_dilation: f64, phase_out: bool) -> DVec3 {
    position_at_effective(start, velocity, acceleration, effective_elapsed(elapsed, time_dilation, phase_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_velocity_uses_physics_timestep() {
        let position = position_at_effective(DVec3::ZERO, Vec3::new(2.0, -1.0, 0.0), Vec3::new(4.0, 2.0, 0.0), 1.0);

        assert_eq!(position, DVec3::new(2.0 + 2.0 * (1.0 - PHYSICS_TIMESTEP), -1.0 + (1.0 - PHYSICS_TIMESTEP), 0.0));
    }

    #[test]
    fn healthy_motion_has_no_interpolation_cap() {
        assert_eq!(effective_elapsed(12.0, 1.0, false), 12.0);
    }

    #[test]
    fn phase_out_tapers_and_stops() {
        assert_eq!(effective_elapsed(2.0, 1.0, true), 2.0);
        assert_eq!(effective_elapsed(2.5, 1.0, true), 2.375);
        assert_eq!(effective_elapsed(3.0, 1.0, true), 2.5);
        assert_eq!(effective_elapsed(12.0, 1.0, true), 2.5);
    }
}
