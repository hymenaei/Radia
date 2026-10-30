use crate::{DVec3, Vec3};
use std::time::{Duration, Instant};

pub const PHYSICS_TIMESTEP: f64 = 1.0 / 45.0;
pub const PHASE_OUT_TIME: f64 = 2.0;
pub const MAX_INTERPOLATION_TIME: f64 = 3.0;
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const MAX_REGION_CROSSING_TIME: f64 = 1.0;
const MAX_REGION_CROSSINGS: usize = 32;
const REGION_SIZE: f64 = 256.0;
const REGION_EDGE_EPSILON: f64 = 0.000_001;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionBounds {
    pub region_id: u64,
    pub origin: DVec3,
}

impl RegionBounds {
    fn contains(self, position: DVec3) -> bool {
        position.x >= self.origin.x
            && position.x < self.origin.x + REGION_SIZE
            && position.y >= self.origin.y
            && position.y < self.origin.y + REGION_SIZE
    }
}

pub fn half_ping_lead(time_dilation: f64, ping_delay: Option<Duration>, frame_dt: f64) -> f64 {
    if !time_dilation.is_finite() || !frame_dt.is_finite() {
        return 0.0;
    }
    0.5 * time_dilation.clamp(0.0, 1.0) * (ping_delay.map_or(0.0, |delay| delay.as_secs_f64()) + frame_dt.max(0.0))
}

pub fn phase_out_needed(sampled_at: Instant, now: Instant, last_packet_received_at: Instant, connected: bool) -> bool {
    now.saturating_duration_since(sampled_at).as_secs_f64() > PHASE_OUT_TIME
        && (!connected || now.saturating_duration_since(last_packet_received_at) > Duration::from_secs_f64(PHASE_OUT_TIME))
}

pub fn effective_elapsed(elapsed: f64, time_dilation: f64) -> f64 {
    let elapsed = if elapsed.is_finite() { elapsed.max(0.0) } else { 0.0 };
    let time_dilation = if time_dilation.is_finite() { time_dilation.clamp(0.0, 1.0) } else { 0.0 };
    elapsed * time_dilation
}

pub fn phase_out_scale(elapsed: f64, phase_out: bool) -> f64 {
    if !phase_out {
        return 1.0;
    }
    if !elapsed.is_finite() || elapsed >= MAX_INTERPOLATION_TIME {
        return 0.0;
    }
    if elapsed <= PHASE_OUT_TIME {
        return 1.0;
    }
    (MAX_INTERPOLATION_TIME - elapsed) / (MAX_INTERPOLATION_TIME - PHASE_OUT_TIME)
}

pub fn position_at_effective(start: DVec3, velocity: Vec3, acceleration: Vec3, elapsed: f64) -> DVec3 {
    start + velocity.as_dvec3() * elapsed + acceleration.as_dvec3() * (0.5 * elapsed * (elapsed - PHYSICS_TIMESTEP))
}

pub fn apply_phase_out(start: DVec3, predicted: DVec3, elapsed: f64, phase_out: bool) -> DVec3 {
    start + (predicted - start) * phase_out_scale(elapsed, phase_out)
}

pub fn linear_position(start: DVec3, velocity: Vec3, acceleration: Vec3, elapsed: f64, time_dilation: f64, phase_out: bool) -> DVec3 {
    let effective = effective_elapsed(elapsed, time_dilation);
    let predicted = position_at_effective(start, velocity, acceleration, effective);
    apply_phase_out(start, predicted, elapsed, phase_out)
}

pub fn bounded_position(
    region_id: u64,
    start: DVec3,
    velocity: Vec3,
    acceleration: Vec3,
    effective_time: f64,
    time_dilation: f64,
    regions: &[RegionBounds],
) -> DVec3 {
    if !start.is_finite() || !velocity.is_finite() || !acceleration.is_finite() || !effective_time.is_finite() {
        return start;
    }
    let effective_time = effective_time.max(0.0);
    let predicted = position_at_effective(start, velocity, acceleration, effective_time);
    let assigned_bounds = regions.iter().copied().find(|bounds| bounds.region_id == region_id);
    let Some(mut bounds) =
        assigned_bounds.filter(|bounds| bounds.contains(start)).or_else(|| regions.iter().copied().find(|bounds| bounds.contains(start)))
    else {
        return if assigned_bounds.is_some() { start } else { predicted };
    };
    let mut after_elapsed = 0.0;
    let mut allowed_elapsed = effective_time;
    let mut last_boundary = start;
    for crossing_index in 0..MAX_REGION_CROSSINGS {
        let Some((crossing_elapsed, point, direction)) =
            first_region_exit(start, velocity, acceleration, bounds, after_elapsed, allowed_elapsed)
        else {
            return if crossing_index == 0 { predicted } else { position_at_effective(start, velocity, acceleration, allowed_elapsed) };
        };
        last_boundary = point;
        let probe = point + direction * REGION_EDGE_EPSILON;
        let Some(next_region) = region_at(probe, regions) else { return point };
        let Some(next_bounds) = regions.iter().copied().find(|candidate| candidate.region_id == next_region) else { return point };
        if crossing_index == 0 {
            let Some(crossing_time) = elapsed_from_effective(crossing_elapsed, time_dilation) else { return point };
            allowed_elapsed = effective_time.min(effective_elapsed(crossing_time + MAX_REGION_CROSSING_TIME, time_dilation));
        }
        bounds = next_bounds;
        after_elapsed = crossing_elapsed + crossing_elapsed.abs().max(1.0) * f64::EPSILON * 8.0;
    }
    last_boundary
}

fn elapsed_from_effective(effective: f64, time_dilation: f64) -> Option<f64> {
    if !time_dilation.is_finite() || time_dilation <= 0.0 {
        return None;
    }
    let phase_elapsed = effective / time_dilation;
    Some(phase_elapsed)
}

fn first_region_exit(
    start: DVec3,
    velocity: Vec3,
    acceleration: Vec3,
    bounds: RegionBounds,
    after_elapsed: f64,
    max_elapsed: f64,
) -> Option<(f64, DVec3, DVec3)> {
    let velocity_d = velocity.as_dvec3();
    let acceleration_d = acceleration.as_dvec3();
    let edges = [
        (start.x, velocity_d.x, acceleration_d.x, bounds.origin.x, -1.0, DVec3::NEG_X),
        (start.x, velocity_d.x, acceleration_d.x, bounds.origin.x + REGION_SIZE, 1.0, DVec3::X),
        (start.y, velocity_d.y, acceleration_d.y, bounds.origin.y, -1.0, DVec3::NEG_Y),
        (start.y, velocity_d.y, acceleration_d.y, bounds.origin.y + REGION_SIZE, 1.0, DVec3::Y),
    ];
    let mut crossing = None;
    for (position, component_velocity, component_acceleration, edge, direction, vector) in edges {
        for time in boundary_roots(edge - position, component_velocity, component_acceleration) {
            if !time.is_finite() || time < after_elapsed || time < 0.0 || time > max_elapsed {
                continue;
            }
            let derivative = component_velocity + component_acceleration * (time - 0.5 * PHYSICS_TIMESTEP);
            if direction * derivative <= 0.0 {
                continue;
            }
            if crossing.is_none_or(|(current, _, _)| time < current) {
                crossing = Some((time, position_at_effective(start, velocity, acceleration, time), vector));
            }
        }
    }
    crossing
}

fn boundary_roots(distance: f64, velocity: f64, acceleration: f64) -> [f64; 2] {
    let a = 0.5 * acceleration;
    let b = velocity - 0.5 * acceleration * PHYSICS_TIMESTEP;
    if a == 0.0 {
        if b == 0.0 {
            return [f64::NAN; 2];
        }
        let time = distance / b;
        return [time, f64::NAN];
    }
    let discriminant = b * b + 4.0 * a * distance;
    if discriminant < 0.0 || !discriminant.is_finite() {
        return [f64::NAN; 2];
    }
    let root = discriminant.sqrt();
    [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)]
}

fn region_at(position: DVec3, regions: &[RegionBounds]) -> Option<u64> {
    regions.iter().find(|bounds| bounds.contains(position)).map(|bounds| bounds.region_id)
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
        assert_eq!(effective_elapsed(12.0, 1.0), 12.0);
    }

    #[test]
    fn phase_out_tapers_and_stops() {
        assert_eq!(phase_out_scale(2.0, true), 1.0);
        assert_eq!(phase_out_scale(2.5, true), 0.5);
        assert_eq!(phase_out_scale(3.0, true), 0.0);
        assert_eq!(phase_out_scale(12.0, true), 0.0);
        assert_eq!(phase_out_scale(12.0, false), 1.0);
        assert_eq!(linear_position(DVec3::ZERO, Vec3::X, Vec3::ZERO, 2.5, 1.0, true), DVec3::X * 1.25);
    }

    #[test]
    fn phase_out_requires_a_silent_or_disconnected_circuit() {
        let sampled_at = Instant::now();
        let now = sampled_at + Duration::from_secs(5);
        assert!(!phase_out_needed(sampled_at, now, now, true));
        assert!(phase_out_needed(sampled_at, now, now - Duration::from_secs(3), true));
        assert!(phase_out_needed(sampled_at, now, now, false));
    }

    #[test]
    fn half_ping_lead_applies_time_dilation_to_ping_and_frame() {
        assert!((half_ping_lead(0.5, Some(Duration::from_millis(100)), 0.02) - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn unmeasured_ping_uses_only_frame_lead() {
        assert!((half_ping_lead(0.5, None, 0.02) - 0.005).abs() < f64::EPSILON);
    }

    #[test]
    fn unloaded_region_edge_stops_prediction() {
        let regions = [RegionBounds { region_id: 1, origin: DVec3::ZERO }];

        let position = bounded_position(1, DVec3::new(255.0, 20.0, 0.0), Vec3::X * 100.0, Vec3::ZERO, 2.0, 1.0, &regions);

        assert_eq!(position.x, 256.0);
    }

    #[test]
    fn loaded_region_crossing_limits_prediction_to_one_second_after_boundary() {
        let regions =
            [RegionBounds { region_id: 1, origin: DVec3::ZERO }, RegionBounds { region_id: 2, origin: DVec3::new(256.0, 0.0, 0.0) }];

        let position = bounded_position(1, DVec3::new(255.0, 20.0, 0.0), Vec3::X * 100.0, Vec3::ZERO, 5.0, 1.0, &regions);

        assert_eq!(position.x, 356.0);
    }
}
