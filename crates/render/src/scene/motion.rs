use std::{collections::HashMap, time::Instant};

use math::{DVec3, Quat, Vec3};
use world::{Entity, Parent, Region, WorldPose, WorldTransform};

use super::SceneView;

const PHYSICS_TIMESTEP: f64 = 1.0 / 45.0;
const PHASE_OUT_TIME: f64 = 2.0;
const MAX_INTERPOLATION_TIME: f64 = 3.0;
const MAX_REGION_CROSSING_TIME: f64 = 1.0;
const REGION_SIZE: f64 = 256.0;
const REGION_EDGE_EPSILON: f64 = 0.000_001;
const MAX_PARENT_EDGES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MotionSample {
    pub(crate) transform: WorldTransform,
    pub(crate) region: Region,
    pub(crate) local_position: Vec3,
    pub(crate) local_rotation: Quat,
    pub(crate) velocity: Vec3,
    pub(crate) acceleration: Vec3,
    pub(crate) angular_velocity: Vec3,
    pub(crate) is_physical: bool,
    pub(crate) time_dilation: f32,
    pub(crate) is_attachment: bool,
    pub(crate) source: Option<world::UpdateSource>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MotionState {
    sampled_at: Instant,
    angular_offset: Quat,
}

impl MotionState {
    pub(crate) fn new(sampled_at: Instant) -> Self {
        Self { sampled_at, angular_offset: Quat::IDENTITY }
    }

    pub(crate) fn updated(previous_state: Self, previous: MotionSample, next: MotionSample, at: Instant) -> Self {
        if previous == next {
            return previous_state;
        }
        let physical_velocity_changed = next.is_physical && previous.angular_velocity != next.angular_velocity;
        let angular_offset = if previous.local_rotation == next.local_rotation && !physical_velocity_changed {
            next.local_rotation.inverse() * local_rotation(previous, previous_state, at)
        } else {
            Quat::IDENTITY
        };
        Self { sampled_at: at, angular_offset }
    }

    fn elapsed(self, now: Instant) -> f64 {
        now.checked_duration_since(self.sampled_at).map_or(0.0, |elapsed| elapsed.as_secs_f64())
    }

    fn local_rotation(self, sample: MotionSample, now: Instant) -> Quat {
        local_rotation(sample, self, now)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RegionBounds {
    region: Region,
    origin: DVec3,
}

impl RegionBounds {
    fn contains(self, position: DVec3) -> bool {
        position.x >= self.origin.x
            && position.x < self.origin.x + REGION_SIZE
            && position.y >= self.origin.y
            && position.y < self.origin.y + REGION_SIZE
    }
}

pub(crate) fn fill_placements(
    view: &SceneView,
    now: Instant,
    entities: &[Entity],
    placements: &mut HashMap<Entity, WorldTransform>,
    regions: &mut Vec<RegionBounds>,
) {
    placements.clear();
    regions.clear();
    regions.extend(view.regions().filter_map(|region| {
        region.summary().info.resolved_origin().map(|origin| RegionBounds { region: region.summary().region, origin })
    }));
    for entity in entities {
        let _ = placement(*entity, view, now, placements, regions, 0);
    }
}

fn placement(
    entity: Entity,
    view: &SceneView,
    now: Instant,
    placements: &mut HashMap<Entity, WorldTransform>,
    regions: &[RegionBounds],
    depth: usize,
) -> Option<WorldTransform> {
    if let Some(transform) = placements.get(&entity) {
        return Some(*transform);
    }
    if depth > MAX_PARENT_EDGES {
        return None;
    }
    let object = view.object(entity)?;
    let base = match object.placement() {
        crate::scene::Placement::Ready(transform) => transform,
        crate::scene::Placement::Hidden(_) => return None,
    };
    let sample = object.motion_sample()?;
    let mut transform = base;
    if sample.is_attachment {
        transform.pose.rotation = object.motion_state().local_rotation(sample, now);
    } else {
        match object.detail().summary.parent {
            Parent::Root => {
                let state = object.motion_state();
                transform.pose =
                    WorldPose { position: linear_position(sample, state, now, regions), rotation: state.local_rotation(sample, now) };
            }
            Parent::Resolved(parent) => {
                let parent_transform = placement(parent, view, now, placements, regions, depth + 1)?;
                transform = child_transform(parent_transform, sample, object.motion_state(), now);
            }
            Parent::Unknown | Parent::Pending { .. } => {}
        }
    }
    placements.insert(entity, transform);
    Some(transform)
}

fn child_transform(parent: WorldTransform, sample: MotionSample, state: MotionState, now: Instant) -> WorldTransform {
    WorldTransform {
        pose: WorldPose {
            position: parent.pose.position + (parent.pose.rotation * sample.local_position).as_dvec3(),
            rotation: parent.pose.rotation * state.local_rotation(sample, now),
        },
        scale: sample.transform.scale,
    }
}

pub(crate) fn transform_changes(current: WorldTransform, next: WorldTransform) -> bool {
    current.pose != next.pose
}

pub(crate) fn should_redraw(visible: bool, current: WorldTransform, next: WorldTransform) -> bool {
    visible && transform_changes(current, next)
}

fn local_rotation(sample: MotionSample, state: MotionState, now: Instant) -> Quat {
    let elapsed = state.elapsed(now) * f64::from(sample.time_dilation);
    sample.local_rotation * state.angular_offset * rotation_delta(sample.angular_velocity, elapsed)
}

fn rotation_delta(angular_velocity: Vec3, elapsed: f64) -> Quat {
    let velocity = angular_velocity.as_dvec3();
    let rate = velocity.length();
    if !rate.is_finite() || rate * rate <= 0.000_01 || !elapsed.is_finite() || elapsed <= 0.0 {
        return Quat::IDENTITY;
    }
    let angle = (rate * elapsed).rem_euclid(std::f64::consts::TAU) as f32;
    Quat::from_axis_angle((velocity / rate).as_vec3(), angle)
}

fn linear_position(sample: MotionSample, state: MotionState, now: Instant, regions: &[RegionBounds]) -> DVec3 {
    let start = sample.transform.pose.position;
    if sample.is_attachment || (sample.velocity == Vec3::ZERO && sample.acceleration == Vec3::ZERO) {
        return start;
    }
    let elapsed = state.elapsed(now);
    let time_dilation = f64::from(sample.time_dilation);
    let effective = effective_elapsed(elapsed, time_dilation);
    let predicted = position_at_effective(sample, effective);
    let Some(bounds) = regions.iter().copied().find(|bounds| bounds.region == sample.region) else { return predicted };
    let candidate_region = region_at(predicted, regions);
    let crossing = first_region_exit(sample, bounds, effective);
    if candidate_region != Some(sample.region) {
        let Some((crossing_time, point, direction)) = crossing else { return clamp_to_region(predicted, bounds) };
        let probe = point + direction * REGION_EDGE_EPSILON;
        if region_at(probe, regions).is_none() {
            return point;
        }
        let Some(crossing_elapsed) = raw_elapsed(crossing_time, time_dilation) else { return point };
        if elapsed > crossing_elapsed + MAX_REGION_CROSSING_TIME {
            let allowed = effective_elapsed(crossing_elapsed + MAX_REGION_CROSSING_TIME, time_dilation);
            let stopped = position_at_effective(sample, allowed);
            return if region_at(stopped, regions).is_some() { stopped } else { point };
        }
    }
    predicted
}

fn position_at_effective(sample: MotionSample, elapsed: f64) -> DVec3 {
    let velocity = sample.velocity.as_dvec3();
    let acceleration = sample.acceleration.as_dvec3();
    sample.transform.pose.position + velocity * elapsed + acceleration * (0.5 * elapsed * (elapsed - PHYSICS_TIMESTEP))
}

fn effective_elapsed(elapsed: f64, time_dilation: f64) -> f64 {
    let elapsed = elapsed.max(0.0);
    let phase_elapsed = if elapsed <= PHASE_OUT_TIME {
        elapsed
    } else if elapsed < MAX_INTERPOLATION_TIME {
        let phase = elapsed - PHASE_OUT_TIME;
        PHASE_OUT_TIME + phase - 0.5 * phase * phase
    } else {
        PHASE_OUT_TIME + 0.5
    };
    phase_elapsed * time_dilation
}

fn raw_elapsed(effective: f64, time_dilation: f64) -> Option<f64> {
    if !time_dilation.is_finite() || time_dilation <= 0.0 {
        return None;
    }
    let phase_elapsed = effective / time_dilation;
    if phase_elapsed <= PHASE_OUT_TIME {
        Some(phase_elapsed)
    } else if phase_elapsed <= PHASE_OUT_TIME + 0.5 {
        Some(MAX_INTERPOLATION_TIME - (5.0 - 2.0 * phase_elapsed).sqrt())
    } else {
        None
    }
}

fn first_region_exit(sample: MotionSample, bounds: RegionBounds, max_effective: f64) -> Option<(f64, DVec3, DVec3)> {
    let start = sample.transform.pose.position;
    let velocity = sample.velocity.as_dvec3();
    let acceleration = sample.acceleration.as_dvec3();
    let edges = [
        (start.x, velocity.x, acceleration.x, bounds.origin.x, -1.0, DVec3::NEG_X),
        (start.x, velocity.x, acceleration.x, bounds.origin.x + REGION_SIZE, 1.0, DVec3::X),
        (start.y, velocity.y, acceleration.y, bounds.origin.y, -1.0, DVec3::NEG_Y),
        (start.y, velocity.y, acceleration.y, bounds.origin.y + REGION_SIZE, 1.0, DVec3::Y),
    ];
    let mut crossing = None;
    for (position, velocity, acceleration, edge, direction, vector) in edges {
        for time in boundary_roots(edge - position, velocity, acceleration) {
            if !time.is_finite() || time < 0.0 || time > max_effective {
                continue;
            }
            let derivative = velocity + acceleration * (time - 0.5 * PHYSICS_TIMESTEP);
            if direction * derivative <= 0.0 {
                continue;
            }
            if crossing.is_none_or(|(current, _, _)| time < current) {
                crossing = Some((time, position_at_effective(sample, time), vector));
            }
        }
    }
    crossing
}

fn boundary_roots(distance: f64, velocity: f64, acceleration: f64) -> [f64; 2] {
    let a = 0.5 * acceleration;
    let b = velocity - 0.5 * acceleration * PHYSICS_TIMESTEP;
    let c = -distance;
    if a == 0.0 {
        let root = if b == 0.0 { f64::NAN } else { -c / b };
        return [root, f64::NAN];
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 || !discriminant.is_finite() {
        return [f64::NAN; 2];
    }
    let root = discriminant.sqrt();
    [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)]
}

fn region_at(position: DVec3, regions: &[RegionBounds]) -> Option<Region> {
    regions.iter().find(|bounds| bounds.contains(position)).map(|bounds| bounds.region)
}

fn clamp_to_region(position: DVec3, bounds: RegionBounds) -> DVec3 {
    DVec3::new(
        position.x.clamp(bounds.origin.x, bounds.origin.x + REGION_SIZE - REGION_EDGE_EPSILON),
        position.y.clamp(bounds.origin.y, bounds.origin.y + REGION_SIZE - REGION_EDGE_EPSILON),
        position.z,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample(position: DVec3, velocity: Vec3, acceleration: Vec3, angular_velocity: Vec3, is_physical: bool) -> MotionSample {
        MotionSample {
            transform: WorldTransform { pose: WorldPose { position, rotation: Quat::IDENTITY }, scale: Vec3::ONE },
            region: Region::new(1),
            local_position: Vec3::ZERO,
            local_rotation: Quat::IDENTITY,
            velocity,
            acceleration,
            angular_velocity,
            is_physical,
            time_dilation: 1.0,
            is_attachment: false,
            source: None,
        }
    }

    fn close_vec(actual: DVec3, expected: DVec3) {
        assert!(actual.distance(expected) < 0.000_01, "expected {expected:?}, got {actual:?}");
    }

    fn close_rotation(actual: Quat, expected: Quat) {
        assert!(actual.dot(expected).abs() > 0.999_99, "expected {expected:?}, got {actual:?}");
    }

    #[test]
    fn acceleration_uses_average_velocity_timestep() {
        let now = Instant::now();
        let sample = sample(DVec3::ZERO, Vec3::new(2.0, -1.0, 0.0), Vec3::new(4.0, 2.0, 0.0), Vec3::ZERO, false);
        let position = position_at_effective(sample, 1.0);
        let expected = DVec3::new(2.0 + 2.0 * (1.0 - PHYSICS_TIMESTEP), -1.0 + (1.0 - PHYSICS_TIMESTEP), 0.0);
        assert_eq!(effective_elapsed(1.0, 1.0), 1.0);
        close_vec(position, expected);
        assert_eq!(MotionState::new(now).elapsed(now), 0.0);
    }

    #[test]
    fn interpolation_phases_out_and_stops() {
        assert_eq!(effective_elapsed(2.0, 1.0), 2.0);
        assert_eq!(effective_elapsed(2.5, 1.0), 2.375);
        assert_eq!(effective_elapsed(3.0, 1.0), 2.5);
        assert_eq!(effective_elapsed(12.0, 1.0), 2.5);
    }

    #[test]
    fn omega_uses_axis_and_rate() {
        let now = Instant::now();
        let sample = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z * 2.0, false);
        let state = MotionState::new(now);
        close_rotation(state.local_rotation(sample, now + Duration::from_millis(500)), Quat::from_rotation_z(1.0));
    }

    #[test]
    fn update_rebases_position_and_keeps_target_spin() {
        let now = Instant::now();
        let old = sample(DVec3::ZERO, Vec3::X * 2.0, Vec3::ZERO, Vec3::Z, false);
        let old_state = MotionState::new(now);
        let update_at = now + Duration::from_millis(500);
        let old_position = position_at_effective(old, old_state.elapsed(update_at));
        close_vec(old_position, DVec3::X);
        let next = sample(DVec3::X * 5.0, Vec3::X * 4.0, Vec3::ZERO, Vec3::Z, false);
        let next_state = MotionState::updated(old_state, old, next, update_at);
        close_vec(position_at_effective(next, next_state.elapsed(update_at) + 0.5), DVec3::X * 7.0);
        close_rotation(next_state.local_rotation(next, update_at), Quat::from_rotation_z(0.5));
    }

    #[test]
    fn stopped_motion_stays_stopped() {
        let now = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::X, Vec3::ZERO, Vec3::Z, false);
        let moving_state = MotionState::new(now);
        let stopped_at = now + Duration::from_secs(1);
        let stopped_position = position_at_effective(moving, 1.0);
        let stopped_rotation = moving_state.local_rotation(moving, stopped_at);
        let mut stopped = sample(stopped_position, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, false);
        stopped.local_rotation = stopped_rotation;
        let stopped_state = MotionState::updated(moving_state, moving, stopped, stopped_at);
        close_vec(position_at_effective(stopped, stopped_state.elapsed(stopped_at) + 2.0), stopped_position);
        close_rotation(stopped_state.local_rotation(stopped, stopped_at + Duration::from_secs(2)), stopped_rotation);
    }

    #[test]
    fn zero_velocity_stops_linear_projection() {
        let now = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::X, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let update_at = now + Duration::from_secs(1);
        let stopped = sample(DVec3::X, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, false);
        let next_state = MotionState::updated(state, moving, stopped, update_at);
        close_vec(linear_position(stopped, next_state, update_at + Duration::from_secs(2), &[]), DVec3::X);
    }

    #[test]
    fn zero_omega_stops_local_rotation() {
        let now = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z, false);
        let state = MotionState::new(now);
        let update_at = now + Duration::from_secs(1);
        let mut stopped = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, false);
        stopped.local_rotation = state.local_rotation(moving, update_at);
        let next_state = MotionState::updated(state, moving, stopped, update_at);
        close_rotation(next_state.local_rotation(stopped, update_at + Duration::from_secs(2)), stopped.local_rotation);
    }

    #[test]
    fn attachment_spins_without_linear_extrapolation() {
        let now = Instant::now();
        let mut attachment = sample(DVec3::X, Vec3::X, Vec3::ZERO, Vec3::Z, false);
        attachment.is_attachment = true;
        let state = MotionState::new(now);
        close_vec(linear_position(attachment, state, now + Duration::from_secs(1), &[]), DVec3::X);
        close_rotation(state.local_rotation(attachment, now + Duration::from_millis(500)), Quat::from_rotation_z(0.5));
    }

    #[test]
    fn physical_rate_update_resets_spin_offset() {
        let now = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z, true);
        let state = MotionState::new(now);
        let at = now + Duration::from_secs(1);
        let mut update = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z * 2.0, true);
        update.source = Some(world::UpdateSource::new(Region::new(1), 2, world::OrderEvidence::Ordered(2), 0));
        let updated = MotionState::updated(state, moving, update, at);
        close_rotation(updated.local_rotation(update, at), Quat::IDENTITY);
    }

    #[test]
    fn authoritative_rotation_resets_spin_offset() {
        let now = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z, false);
        let state = MotionState::new(now);
        let at = now + Duration::from_secs(1);
        let mut update = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z, false);
        update.local_rotation = Quat::from_rotation_x(0.25);
        let updated = MotionState::updated(state, moving, update, at);
        close_rotation(updated.local_rotation(update, at), update.local_rotation);
    }

    #[test]
    fn linkset_children_follow_root_rotation() {
        let now = Instant::now();
        let root = sample(DVec3::ZERO, Vec3::X * 2.0, Vec3::ZERO, Vec3::Z * std::f32::consts::FRAC_PI_2, false);
        let root_state = MotionState::new(now);
        let child_time = now + Duration::from_secs(1);
        let root_transform = WorldTransform {
            pose: WorldPose {
                position: linear_position(root, root_state, child_time, &[]),
                rotation: root_state.local_rotation(root, child_time),
            },
            scale: Vec3::ONE,
        };
        let mut child = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, false);
        child.local_position = Vec3::X * 2.0;
        let child_transform = child_transform(root_transform, child, MotionState::new(now), child_time);
        close_vec(child_transform.pose.position, DVec3::new(2.0, 2.0, 0.0));
        close_rotation(child_transform.pose.rotation, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
    }

    #[test]
    fn redraw_stops_for_stationary_or_invisible_transforms() {
        let stationary = WorldTransform { pose: WorldPose { position: DVec3::ZERO, rotation: Quat::IDENTITY }, scale: Vec3::ONE };
        let moving = WorldTransform { pose: WorldPose { position: DVec3::X, rotation: Quat::IDENTITY }, scale: Vec3::ONE };
        let spinning = WorldTransform { pose: WorldPose { position: DVec3::ZERO, rotation: Quat::from_rotation_z(0.1) }, scale: Vec3::ONE };
        assert!(should_redraw(true, stationary, moving));
        assert!(should_redraw(true, stationary, spinning));
        assert!(!should_redraw(true, stationary, stationary));
        assert!(!should_redraw(false, stationary, moving));
        assert!(!should_redraw(false, stationary, spinning));
    }

    #[test]
    fn region_crossing_stops_at_one_second() {
        let now = Instant::now();
        let moving = sample(DVec3::new(255.0, 20.0, 0.0), Vec3::X * 100.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [
            RegionBounds { region: Region::new(1), origin: DVec3::ZERO },
            RegionBounds { region: Region::new(2), origin: DVec3::new(256.0, 0.0, 0.0) },
        ];
        close_vec(linear_position(moving, state, now + Duration::from_millis(1500), &regions), DVec3::new(356.0, 20.0, 0.0));
    }

    #[test]
    fn missing_neighbor_clips_at_region_edge() {
        let now = Instant::now();
        let moving = sample(DVec3::new(255.0, 20.0, 0.0), Vec3::X * 100.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [RegionBounds { region: Region::new(1), origin: DVec3::ZERO }];
        close_vec(linear_position(moving, state, now + Duration::from_millis(1500), &regions), DVec3::new(256.0, 20.0, 0.0));
    }

}
