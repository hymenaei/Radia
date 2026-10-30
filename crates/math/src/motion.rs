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
const PHASE_OUT_DURATION: f64 = MAX_INTERPOLATION_TIME - PHASE_OUT_TIME;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearTrajectory {
    start: DVec3,
    velocity: Vec3,
    acceleration: Vec3,
    time_dilation: f64,
    sampled_at: Instant,
    phase_out_at: Option<Instant>,
}

impl LinearTrajectory {
    pub fn new(start: DVec3, velocity: Vec3, acceleration: Vec3, time_dilation: f64, sampled_at: Instant) -> Self {
        Self { start, velocity, acceleration, time_dilation, sampled_at, phase_out_at: None }
    }

    pub fn start_phase_out(&mut self, at: Instant) {
        self.phase_out_at.get_or_insert(at.max(self.sampled_at));
    }

    pub fn is_phase_out_settled(self, now: Instant) -> bool {
        self.phase_out_at.is_some_and(|at| now.saturating_duration_since(at).as_secs_f64() >= PHASE_OUT_DURATION)
    }

    pub fn position_at(self, region_id: u64, now: Instant, regions: &[RegionBounds]) -> DVec3 {
        let Some(phase_out_at) = self.phase_out_at.filter(|at| now >= *at) else {
            return self.normal_position(region_id, now, regions);
        };
        let taper_elapsed = now.saturating_duration_since(phase_out_at).as_secs_f64().min(PHASE_OUT_DURATION);
        let phase_start = self.normal_position(region_id, phase_out_at, regions);
        if taper_elapsed == 0.0 {
            return phase_start;
        }
        let effective_to_phase =
            effective_elapsed(phase_out_at.saturating_duration_since(self.sampled_at).as_secs_f64(), self.time_dilation);
        let dilation = if self.time_dilation.is_finite() { self.time_dilation.clamp(0.0, 1.0) } else { 0.0 };
        let velocity = self.velocity.as_dvec3() + self.acceleration.as_dvec3() * (effective_to_phase - 0.5 * PHYSICS_TIMESTEP);
        let wall_velocity = velocity * dilation;
        let wall_acceleration = self.acceleration.as_dvec3() * (dilation * dilation);
        let first_integral = taper_elapsed - taper_elapsed * taper_elapsed / (2.0 * PHASE_OUT_DURATION);
        let second_integral =
            taper_elapsed * taper_elapsed / 2.0 - taper_elapsed * taper_elapsed * taper_elapsed / (3.0 * PHASE_OUT_DURATION);
        let displacement = wall_velocity * first_integral + wall_acceleration * second_integral;
        let average_velocity = (displacement / taper_elapsed).as_vec3();
        bounded_position(region_id, phase_start, average_velocity, Vec3::ZERO, taper_elapsed, 1.0, regions)
    }

    pub fn resume_at(&mut self, region_id: u64, now: Instant, regions: &[RegionBounds]) {
        if self.phase_out_at.is_none() {
            return;
        }
        let position = self.position_at(region_id, now, regions);
        let effective = effective_elapsed(now.saturating_duration_since(self.sampled_at).as_secs_f64(), self.time_dilation);
        let velocity = self.velocity + self.acceleration * effective as f32;
        *self = Self::new(position, velocity, self.acceleration, self.time_dilation, now);
    }

    fn normal_position(self, region_id: u64, now: Instant, regions: &[RegionBounds]) -> DVec3 {
        let effective = effective_elapsed(now.saturating_duration_since(self.sampled_at).as_secs_f64(), self.time_dilation);
        bounded_position(region_id, self.start, self.velocity, self.acceleration, effective, self.time_dilation, regions)
    }
}

pub fn half_ping_lead(time_dilation: f64, ping_delay: Option<Duration>, frame_dt: f64) -> f64 {
    if !time_dilation.is_finite() || !frame_dt.is_finite() {
        return 0.0;
    }
    0.5 * time_dilation.clamp(0.0, 1.0) * (ping_delay.map_or(0.0, |delay| delay.as_secs_f64()) + frame_dt.max(0.0))
}

pub fn circuit_active(now: Instant, last_packet_received_at: Instant, connected: bool) -> bool {
    connected && now.saturating_duration_since(last_packet_received_at) <= Duration::from_secs_f64(PHASE_OUT_TIME)
}

pub fn effective_elapsed(elapsed: f64, time_dilation: f64) -> f64 {
    let elapsed = if elapsed.is_finite() { elapsed.max(0.0) } else { 0.0 };
    let time_dilation = if time_dilation.is_finite() { time_dilation.clamp(0.0, 1.0) } else { 0.0 };
    elapsed * time_dilation
}

pub fn position_at_effective(start: DVec3, velocity: Vec3, acceleration: Vec3, elapsed: f64) -> DVec3 {
    start + velocity.as_dvec3() * elapsed + acceleration.as_dvec3() * (0.5 * elapsed * (elapsed - PHYSICS_TIMESTEP))
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
    fn phase_out_is_continuous_at_start() {
        let sampled_at = Instant::now();
        let phase_out_at = sampled_at + Duration::from_secs_f64(PHASE_OUT_TIME);
        let mut trajectory = LinearTrajectory::new(DVec3::ZERO, Vec3::X, Vec3::ZERO, 1.0, sampled_at);
        trajectory.start_phase_out(phase_out_at);

        let before = trajectory.position_at(1, phase_out_at - Duration::from_nanos(1), &[]);
        let at = trajectory.position_at(1, phase_out_at, &[]);

        assert_eq!(at, DVec3::X * PHASE_OUT_TIME);
        assert!(before.distance(at) < 1.0e-8);
    }

    #[test]
    fn phase_out_moves_forward_without_reversing() {
        let sampled_at = Instant::now();
        let phase_out_at = sampled_at + Duration::from_secs_f64(PHASE_OUT_TIME);
        let mut trajectory = LinearTrajectory::new(DVec3::ZERO, Vec3::X, Vec3::ZERO, 1.0, sampled_at);
        trajectory.start_phase_out(phase_out_at);
        let mut previous = trajectory.position_at(1, phase_out_at, &[]).x;

        for elapsed in [0.25, 0.5, 0.75, PHASE_OUT_DURATION, 2.0] {
            let next = trajectory.position_at(1, phase_out_at + Duration::from_secs_f64(elapsed), &[]).x;
            assert!(next >= previous);
            previous = next;
        }
        assert_eq!(previous, PHASE_OUT_TIME + PHASE_OUT_DURATION / 2.0);
    }

    #[test]
    fn accelerated_taper_preserves_velocity() {
        let sampled_at = Instant::now();
        let phase_out_at = sampled_at + Duration::from_secs_f64(PHASE_OUT_TIME);
        let velocity = Vec3::X * 2.0;
        let acceleration = Vec3::X * 4.0;
        let mut trajectory = LinearTrajectory::new(DVec3::ZERO, velocity, acceleration, 1.0, sampled_at);
        trajectory.start_phase_out(phase_out_at);

        let phase_start = trajectory.position_at(1, phase_out_at, &[]);
        let slope_interval = Duration::from_secs_f64(PHYSICS_TIMESTEP / 1_000.0);
        let after_start = trajectory.position_at(1, phase_out_at + slope_interval, &[]);
        let slope = (after_start - phase_start).as_vec3() / slope_interval.as_secs_f64() as f32;
        let elapsed = phase_out_at.duration_since(sampled_at).as_secs_f64();
        let untapered_velocity = velocity + acceleration * (elapsed - 0.5 * PHYSICS_TIMESTEP) as f32;

        assert!((slope - untapered_velocity).length() < 0.001);

        let recovery_at = phase_out_at + Duration::from_secs_f64(PHASE_OUT_DURATION / 2.0);
        let displayed = trajectory.position_at(1, recovery_at, &[]);
        trajectory.resume_at(1, recovery_at, &[]);
        let continuation = Duration::from_secs_f64(PHYSICS_TIMESTEP);
        let continuation_elapsed = continuation.as_secs_f64();
        let recovery_elapsed = recovery_at.duration_since(sampled_at).as_secs_f64();
        let recovery_velocity = velocity + acceleration * (recovery_elapsed - 0.5 * PHYSICS_TIMESTEP) as f32;
        let expected = displayed
            + recovery_velocity.as_dvec3() * continuation_elapsed
            + acceleration.as_dvec3() * (0.5 * continuation_elapsed * continuation_elapsed);

        assert!((trajectory.position_at(1, recovery_at + continuation, &[]) - expected).length() < 1.0e-6);
    }

    #[test]
    fn recovery_rebases_from_displayed_position() {
        let sampled_at = Instant::now();
        let phase_out_at = sampled_at + Duration::from_secs_f64(PHASE_OUT_TIME);
        let recovered_at = phase_out_at + Duration::from_millis(500);
        let mut trajectory = LinearTrajectory::new(DVec3::ZERO, Vec3::X, Vec3::ZERO, 1.0, sampled_at);
        trajectory.start_phase_out(phase_out_at);
        let displayed = trajectory.position_at(1, recovered_at, &[]);

        trajectory.resume_at(1, recovered_at, &[]);

        assert_eq!(trajectory.position_at(1, recovered_at, &[]), displayed);
        assert_eq!(trajectory.position_at(1, recovered_at + Duration::from_millis(100), &[]), displayed + DVec3::X * 0.1);
    }

    #[test]
    fn old_sample_does_not_snap_when_taper_starts() {
        let sampled_at = Instant::now();
        let phase_out_at = sampled_at + Duration::from_secs(4);
        let mut trajectory = LinearTrajectory::new(DVec3::ZERO, Vec3::X, Vec3::ZERO, 1.0, sampled_at);
        let before = trajectory.position_at(1, phase_out_at, &[]);
        trajectory.start_phase_out(phase_out_at);

        assert_eq!(trajectory.position_at(1, phase_out_at, &[]), before);
        assert!(trajectory.position_at(1, phase_out_at + Duration::from_nanos(1), &[]).x >= before.x);
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
