use std::{collections::HashMap, time::Instant};

use math::{DVec3, Quat, Vec3, motion as kinematics};
use world::{Entity, MotionUpdateKind, Parent, Region, WorldPose, WorldTransform};

use super::SceneView;

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
    pub(crate) motion_update_kind: MotionUpdateKind,
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
        if same_motion(previous, next) {
            return previous_state;
        }
        if next.motion_update_kind == MotionUpdateKind::Full
            && previous.local_rotation == next.local_rotation
            && next.angular_velocity == Vec3::ZERO
        {
            return Self::new(at);
        }
        let physical_velocity_changed = next.is_physical && previous.angular_velocity != next.angular_velocity;
        let angular_offset = if previous.local_rotation == next.local_rotation && !physical_velocity_changed {
            local_rotation(previous, previous_state, at) * next.local_rotation.inverse()
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

fn same_motion(previous: MotionSample, next: MotionSample) -> bool {
    previous.transform.pose == next.transform.pose
        && previous.region == next.region
        && previous.local_position == next.local_position
        && previous.local_rotation == next.local_rotation
        && previous.velocity == next.velocity
        && previous.acceleration == next.acceleration
        && previous.angular_velocity == next.angular_velocity
        && previous.is_physical == next.is_physical
        && previous.time_dilation == next.time_dilation
        && previous.motion_update_kind == next.motion_update_kind
        && previous.is_attachment == next.is_attachment
        && previous.source == next.source
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

pub(crate) fn needs_evaluation(view: &SceneView, entity: Entity) -> bool {
    motion_in_chain(view, entity, 0)
}

fn motion_in_chain(view: &SceneView, entity: Entity, depth: usize) -> bool {
    if depth > MAX_PARENT_EDGES {
        return false;
    }
    let Some(object) = view.object(entity) else { return false };
    let Some(sample) = object.motion_sample() else { return false };
    if sample.velocity != Vec3::ZERO || sample.acceleration != Vec3::ZERO || sample.angular_velocity != Vec3::ZERO {
        return true;
    }
    match object.detail().summary.parent {
        Parent::Resolved(parent) => motion_in_chain(view, parent, depth + 1),
        Parent::Root | Parent::Unknown | Parent::Pending { .. } => false,
    }
}

pub(crate) fn fill_placement_pair(
    view: &SceneView,
    now: Instant,
    future: Instant,
    entities: &[Entity],
    placements: &mut HashMap<Entity, WorldTransform>,
    future_placements: &mut HashMap<Entity, WorldTransform>,
    regions: &mut Vec<RegionBounds>,
) {
    placements.clear();
    future_placements.clear();
    regions.clear();
    regions.extend(view.regions().filter_map(|region| {
        region.summary().info.resolved_origin().map(|origin| RegionBounds { region: region.summary().region, origin })
    }));
    for entity in entities {
        let _ = placement_pair(*entity, view, now, future, placements, future_placements, regions, 0);
    }
}

fn placement_pair(
    entity: Entity,
    view: &SceneView,
    now: Instant,
    future: Instant,
    placements: &mut HashMap<Entity, WorldTransform>,
    future_placements: &mut HashMap<Entity, WorldTransform>,
    regions: &[RegionBounds],
    depth: usize,
) -> Option<(WorldTransform, WorldTransform)> {
    if let (Some(current), Some(future)) = (placements.get(&entity), future_placements.get(&entity)) {
        return Some((*current, *future));
    }
    if depth > MAX_PARENT_EDGES {
        return None;
    }
    let object = view.object(entity)?;
    let sample = object.motion_sample()?;
    let mut current = sample.transform;
    let mut next = sample.transform;
    if sample.is_attachment {
        let state = object.motion_state();
        current.pose.rotation = state.local_rotation(sample, now);
        next.pose.rotation = state.local_rotation(sample, future);
    } else {
        match object.detail().summary.parent {
            Parent::Root => {
                let state = object.motion_state();
                current.pose =
                    WorldPose { position: linear_position(sample, state, now, regions), rotation: state.local_rotation(sample, now) };
                next.pose =
                    WorldPose { position: linear_position(sample, state, future, regions), rotation: state.local_rotation(sample, future) };
            }
            Parent::Resolved(parent) => {
                let (parent_current, parent_future) =
                    placement_pair(parent, view, now, future, placements, future_placements, regions, depth + 1)?;
                let state = object.motion_state();
                current = child_transform(parent_current, sample, state, now);
                next = child_transform(parent_future, sample, state, future);
            }
            Parent::Unknown | Parent::Pending { .. } => {}
        }
    }
    placements.insert(entity, current);
    future_placements.insert(entity, next);
    Some((current, next))
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
    rotation_delta(sample.angular_velocity, elapsed) * state.angular_offset * sample.local_rotation
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
    let effective = kinematics::effective_elapsed(elapsed, time_dilation, true);
    let predicted = position_at_effective(sample, effective);
    let assigned_bounds = regions.iter().copied().find(|bounds| bounds.region == sample.region);
    let Some(mut bounds) =
        assigned_bounds.filter(|bounds| bounds.contains(start)).or_else(|| regions.iter().copied().find(|bounds| bounds.contains(start)))
    else {
        return if assigned_bounds.is_some() { start } else { predicted };
    };
    let mut after_effective = 0.0;
    let mut allowed_effective = effective;
    let mut last_boundary = start;
    let max_crossings = regions.len().saturating_mul(8).saturating_add(1);
    for crossing_index in 0..max_crossings {
        let Some((crossing_time, point, direction)) = first_region_exit(sample, bounds, after_effective, allowed_effective) else {
            return if crossing_index == 0 { predicted } else { position_at_effective(sample, allowed_effective) };
        };
        last_boundary = point;
        let probe = point + direction * REGION_EDGE_EPSILON;
        let Some(next_region) = region_at(probe, regions) else { return point };
        let Some(next_bounds) = regions.iter().copied().find(|candidate| candidate.region == next_region) else { return point };
        if crossing_index == 0 {
            let Some(crossing_elapsed) = raw_elapsed(crossing_time, time_dilation) else { return point };
            allowed_effective =
                effective.min(kinematics::effective_elapsed(crossing_elapsed + MAX_REGION_CROSSING_TIME, time_dilation, true));
        }
        bounds = next_bounds;
        after_effective = crossing_time + crossing_time.abs().max(1.0) * f64::EPSILON * 8.0;
    }
    last_boundary
}

fn position_at_effective(sample: MotionSample, elapsed: f64) -> DVec3 {
    kinematics::position_at_effective(sample.transform.pose.position, sample.velocity, sample.acceleration, elapsed)
}

fn raw_elapsed(effective: f64, time_dilation: f64) -> Option<f64> {
    if !time_dilation.is_finite() || time_dilation <= 0.0 {
        return None;
    }
    let phase_elapsed = effective / time_dilation;
    if phase_elapsed <= kinematics::PHASE_OUT_TIME {
        Some(phase_elapsed)
    } else if phase_elapsed <= kinematics::PHASE_OUT_TIME + 0.5 {
        Some(kinematics::MAX_INTERPOLATION_TIME - (5.0 - 2.0 * phase_elapsed).sqrt())
    } else {
        None
    }
}

fn first_region_exit(sample: MotionSample, bounds: RegionBounds, after_effective: f64, max_effective: f64) -> Option<(f64, DVec3, DVec3)> {
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
            if !time.is_finite() || time < after_effective || time < 0.0 || time > max_effective {
                continue;
            }
            let derivative = velocity + acceleration * (time - 0.5 * kinematics::PHYSICS_TIMESTEP);
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
    let b = velocity - 0.5 * acceleration * kinematics::PHYSICS_TIMESTEP;
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

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, time::Duration};

    use uuid::Uuid;
    use world::{LinearMotion, ObjectObservation, OrderEvidence, ParentUpdate, Pose, RegionInfo, UpdateSource, World, WorldLimits};

    use crate::scene::{Budget, ReadFailure, ReadQuery, ReadReply, ReconcileStep, Reconciler, SceneView};

    use super::*;

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
            motion_update_kind: MotionUpdateKind::Full,
            is_attachment: false,
            source: None,
        }
    }

    fn child_scene(parent_scale: Option<Vec3>) -> (Reconciler, SceneView) {
        let limits = WorldLimits {
            max_region_registrations: 4,
            max_entities: 16,
            max_entities_per_region: 16,
            max_tombstones_per_region: 16,
            max_tombstones: 16,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(16),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
            max_journal_entries: 256,
            max_journal_changes: 512,
            max_region_name_bytes: 256,
            max_recovery_targets: 8,
        };
        let mut world = World::new(limits);
        let region = Region::new(1);
        let _ = world.adopt_region(region).expect("region adoption");
        let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region origin");
        let source = |order| UpdateSource::new(region, order as u32, OrderEvidence::Ordered(order), 0);
        let mut root =
            ObjectObservation::new(source(1), Uuid::from_u128(1), NonZeroU32::new(1).unwrap(), world::Kind::Object, ParentUpdate::Root);
        root.pose = Some(Pose { position: Some(Vec3::new(20.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        root.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X * 2.0), acceleration: None });
        root.scale = parent_scale;
        let _ = world.observe_object(root).expect("root observation");
        let mut child = ObjectObservation::new(
            source(2),
            Uuid::from_u128(2),
            NonZeroU32::new(2).unwrap(),
            world::Kind::Object,
            ParentUpdate::Local(NonZeroU32::new(1).unwrap()),
        );
        child.pose = Some(Pose { position: Some(Vec3::X * 3.0), rotation: Some(Quat::IDENTITY) });
        child.scale = Some(Vec3::ONE);
        let _ = world.observe_object(child).expect("child observation");

        let (mut scene, mut view) = Reconciler::new(Budget::default());
        let mut now = Instant::now();
        for _ in 0..10_000 {
            match scene.poll(now) {
                ReconcileStep::Read(job) => {
                    let result = match job.query() {
                        ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                    }
                    .map_err(ReadFailure::World);
                    scene.complete_read(job, result, now);
                }
                ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                ReconcileStep::Publish(batch) => view.apply(batch).expect("scene publication"),
                ReconcileStep::Continue => {}
                ReconcileStep::Wait(until) => now = until,
                ReconcileStep::Idle => {
                    assert_eq!(view.object_count(), 2, "both linkset objects are published");
                    return (scene, view);
                }
                ReconcileStep::Asset(_) => panic!("motion fixture unexpectedly requests an asset"),
                ReconcileStep::Closed => panic!("motion fixture scene unexpectedly closes"),
            }
        }
        panic!("scene did not converge: {:?}", scene.status());
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
        let expected = DVec3::new(2.0 + 2.0 * (1.0 - kinematics::PHYSICS_TIMESTEP), -1.0 + (1.0 - kinematics::PHYSICS_TIMESTEP), 0.0);
        assert_eq!(kinematics::effective_elapsed(1.0, 1.0, true), 1.0);
        close_vec(position, expected);
        assert_eq!(MotionState::new(now).elapsed(now), 0.0);
    }

    #[test]
    fn interpolation_phases_out_and_stops() {
        assert_eq!(kinematics::effective_elapsed(2.0, 1.0, true), 2.0);
        assert_eq!(kinematics::effective_elapsed(2.5, 1.0, true), 2.375);
        assert_eq!(kinematics::effective_elapsed(3.0, 1.0, true), 2.5);
        assert_eq!(kinematics::effective_elapsed(12.0, 1.0, true), 2.5);
    }

    #[test]
    fn omega_uses_axis_and_rate() {
        let now = Instant::now();
        let sample = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z * 2.0, false);
        let state = MotionState::new(now);
        close_rotation(state.local_rotation(sample, now + Duration::from_millis(500)), Quat::from_rotation_z(1.0));
    }

    #[test]
    fn omega_uses_parent_frame() {
        let now = Instant::now();
        let mut sample = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z * 2.0, false);
        sample.local_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let state = MotionState::new(now);
        let actual = state.local_rotation(sample, now + Duration::from_millis(250));
        let expected = Quat::from_xyzw(0.685_124_5, 0.174_941, 0.174_941, 0.685_124_5);
        close_rotation(actual, expected);
    }

    #[test]
    fn negative_omega_reverses_rotation() {
        let now = Instant::now();
        let mut sample = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z * -2.0, false);
        sample.local_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let state = MotionState::new(now);
        let actual = state.local_rotation(sample, now + Duration::from_millis(250));
        let expected = Quat::from_xyzw(0.685_124_5, -0.174_941, -0.174_941, 0.685_124_5);
        close_rotation(actual, expected);
    }

    #[test]
    fn update_rebases_position_and_keeps_target_spin() {
        let now = Instant::now();
        let mut old = sample(DVec3::ZERO, Vec3::X * 2.0, Vec3::ZERO, Vec3::Z * 2.0, false);
        old.local_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let old_state = MotionState::new(now);
        let update_at = now + Duration::from_millis(500);
        let old_position = position_at_effective(old, old_state.elapsed(update_at));
        close_vec(old_position, DVec3::X);
        let mut next = sample(DVec3::X * 5.0, Vec3::X * 4.0, Vec3::ZERO, Vec3::Z * 2.0, false);
        next.local_rotation = old.local_rotation;
        let next_state = MotionState::updated(old_state, old, next, update_at);
        close_vec(position_at_effective(next, next_state.elapsed(update_at) + 0.5), DVec3::X * 7.0);
        close_rotation(next_state.local_rotation(next, update_at), Quat::from_xyzw(0.620_544_6, 0.339_005_1, 0.339_005_1, 0.620_544_6));
        close_rotation(
            next_state.local_rotation(next, update_at + Duration::from_millis(500)),
            Quat::from_xyzw(0.382_051_4, 0.595_009_8, 0.595_009_8, 0.382_051_4),
        );
    }

    #[test]
    fn scale_update_keeps_motion_prediction_clock() {
        let now = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::X * 2.0, Vec3::ZERO, Vec3::Z, false);
        let state = MotionState::new(now);
        let update_at = now + Duration::from_millis(500);
        let mut resized = moving;
        resized.transform.scale = Vec3::splat(2.0);
        let resized_state = MotionState::updated(state, moving, resized, update_at);

        assert_eq!(resized_state.elapsed(update_at), 0.5);
        close_vec(linear_position(resized, resized_state, now + Duration::from_secs(1), &[]), DVec3::X * 2.0);
        close_rotation(resized_state.local_rotation(resized, now + Duration::from_secs(1)), Quat::from_rotation_z(1.0));
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
    fn full_zero_omega_resets_rotation_to_base() {
        let received_at = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z, false);
        let state = MotionState::new(received_at);
        let updated_at = received_at + Duration::from_secs(1);
        let mut update = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, false);
        update.source = Some(UpdateSource::new(Region::new(1), 2, OrderEvidence::Ordered(2), 0));
        let updated = MotionState::updated(state, moving, update, updated_at);

        close_rotation(updated.local_rotation(update, updated_at), update.local_rotation);
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
    fn child_spin_uses_parent_frame() {
        let now = Instant::now();
        let parent = WorldTransform {
            pose: WorldPose { position: DVec3::new(10.0, 20.0, 30.0), rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2) },
            scale: Vec3::ONE,
        };
        let mut child = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z * 2.0, false);
        child.local_position = Vec3::Y * 2.0;
        child.local_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let transform = child_transform(parent, child, MotionState::new(now), now + Duration::from_millis(250));

        close_vec(transform.pose.position, DVec3::new(10.0, 20.0, 32.0));
        close_rotation(transform.pose.rotation, Quat::from_xyzw(0.968_912_4, 0.0, 0.247_404, 0.0));
    }

    #[test]
    fn linkset_child_follows_parent_without_drawable_dimensions() {
        for scale in [None, Some(Vec3::ZERO)] {
            let (_scene, view) = child_scene(scale);
            let root = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).expect("root is published");
            let child = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(2)).expect("child is published");
            std::assert_matches!(root.placement(), crate::scene::Placement::Hidden(_));
            let child_entity = child.detail().summary.entity;
            let at = root.motion_state().sampled_at + Duration::from_millis(500);
            let mut placements = HashMap::new();
            let mut future_placements = HashMap::new();
            let mut regions = Vec::new();

            assert!(needs_evaluation(&view, child_entity));
            fill_placement_pair(
                &view,
                at,
                at + Duration::from_millis(16),
                &[child_entity],
                &mut placements,
                &mut future_placements,
                &mut regions,
            );

            let transform = placements.get(&child_entity).expect("child placement follows its parent");
            close_vec(transform.pose.position, DVec3::new(24.0, 0.0, 0.0));
            assert!(future_placements.contains_key(&child_entity));
        }
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
    fn loaded_neighbor_still_clips_at_the_next_missing_region() {
        let now = Instant::now();
        let moving = sample(DVec3::new(255.0, 20.0, 0.0), Vec3::X * 300.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [
            RegionBounds { region: Region::new(1), origin: DVec3::ZERO },
            RegionBounds { region: Region::new(2), origin: DVec3::new(256.0, 0.0, 0.0) },
        ];

        close_vec(linear_position(moving, state, now + Duration::from_millis(900), &regions), DVec3::new(512.0, 20.0, 0.0));
    }

    #[test]
    fn sample_outside_assigned_region_starts_from_loaded_neighbor() {
        let now = Instant::now();
        let moving = sample(DVec3::new(300.0, 20.0, 0.0), Vec3::X * 300.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [
            RegionBounds { region: Region::new(1), origin: DVec3::ZERO },
            RegionBounds { region: Region::new(2), origin: DVec3::new(256.0, 0.0, 0.0) },
        ];

        close_vec(linear_position(moving, state, now + Duration::from_millis(900), &regions), DVec3::new(512.0, 20.0, 0.0));
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
