use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use math::{DVec3, Quat, Vec3, motion as kinematics};
use world::{Entity, MotionUpdateKind, Parent, Region, WorldPose, WorldTransform};

use super::SceneView;

const MAX_PARENT_EDGES: usize = 16;
const MAX_REGION_HEIGHT: f64 = 4096.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionCircuit {
    pub region: Region,
    pub last_packet_received_at: Instant,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MotionSample {
    pub(crate) transform: WorldTransform,
    pub(crate) kind: world::Kind,
    pub(crate) region: Region,
    pub(crate) local_position: Vec3,
    pub(crate) local_rotation: Quat,
    pub(crate) velocity: Vec3,
    pub(crate) acceleration: Vec3,
    pub(crate) angular_velocity: Vec3,
    pub(crate) is_physical: bool,
    pub(crate) time_dilation: f32,
    pub(crate) ping_delay: Option<Duration>,
    pub(crate) motion_update_kind: MotionUpdateKind,
    pub(crate) is_attachment: bool,
    pub(crate) source: Option<world::UpdateSource>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MotionState {
    sampled_at: Instant,
    angular_offset: Quat,
    linear_trajectory: Option<kinematics::LinearTrajectory>,
}

impl MotionState {
    pub(crate) fn new(sampled_at: Instant) -> Self {
        Self { sampled_at, angular_offset: Quat::IDENTITY, linear_trajectory: None }
    }

    #[cfg(test)]
    pub(crate) fn updated(previous_state: Self, previous: MotionSample, next: MotionSample, at: Instant) -> Self {
        Self::updated_at(previous_state, previous, next, at, at)
    }

    pub(crate) fn updated_at(
        previous_state: Self,
        previous: MotionSample,
        next: MotionSample,
        _rendered_at: Instant,
        sampled_at: Instant,
    ) -> Self {
        if same_motion(previous, next) {
            return previous_state;
        }
        if next.motion_update_kind == MotionUpdateKind::Full
            && previous.local_rotation == next.local_rotation
            && next.angular_velocity == Vec3::ZERO
            && next.source.is_some_and(world::UpdateSource::has_explicit_angular_velocity)
        {
            return Self::new(sampled_at);
        }
        let physical_velocity_changed = next.is_physical && previous.angular_velocity != next.angular_velocity;
        let angular_offset = if previous.local_rotation == next.local_rotation && !physical_velocity_changed {
            local_rotation(previous, previous_state, sampled_at) * next.local_rotation.inverse()
        } else {
            Quat::IDENTITY
        };
        Self { sampled_at, angular_offset, linear_trajectory: Some(linear_trajectory(next, sampled_at)) }
    }

    fn elapsed(self, now: Instant) -> f64 {
        now.checked_duration_since(self.sampled_at).map_or(0.0, |elapsed| elapsed.as_secs_f64())
    }

    fn local_rotation(self, sample: MotionSample, now: Instant) -> Quat {
        local_rotation(sample, self, now)
    }

    pub(in crate::scene) fn start_linear_phase_out(&mut self, sample: MotionSample, at: Option<Instant>) {
        let mut trajectory = self.linear_trajectory.unwrap_or_else(|| linear_trajectory(sample, self.sampled_at));
        let phase_out_at = at.map_or(self.sampled_at, |at| at.max(self.sampled_at));
        trajectory.start_phase_out(phase_out_at.max(self.sampled_at + Duration::from_secs_f64(kinematics::PHASE_OUT_TIME)));
        self.linear_trajectory = Some(trajectory);
    }

    pub(in crate::scene) fn resume_linear_at(&mut self, sample: MotionSample, now: Instant, regions: &[kinematics::RegionBounds]) {
        let Some(mut trajectory) = self.linear_trajectory else { return };
        trajectory.resume_at(sample.region.get(), now, regions);
        self.linear_trajectory = Some(trajectory);
    }

    fn linear_position(self, sample: MotionSample, now: Instant, regions: &[kinematics::RegionBounds]) -> DVec3 {
        self.linear_trajectory.unwrap_or_else(|| linear_trajectory(sample, self.sampled_at)).position_at(sample.region.get(), now, regions)
    }
}

fn linear_trajectory(sample: MotionSample, sampled_at: Instant) -> kinematics::LinearTrajectory {
    let frame_dt = if sample.source.and_then(world::UpdateSource::motion_received_at).is_some() {
        kinematics::FRAME_INTERVAL.as_secs_f64()
    } else {
        0.0
    };
    let lead = kinematics::half_ping_lead(f64::from(sample.time_dilation), sample.ping_delay, frame_dt);
    kinematics::LinearTrajectory::new(
        sample.transform.pose.position + sample.velocity.as_dvec3() * lead,
        sample.velocity,
        sample.acceleration,
        f64::from(sample.time_dilation),
        sampled_at,
    )
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

pub(in crate::scene) fn motion_changed(previous: MotionSample, next: MotionSample) -> bool {
    !same_motion(previous, next)
}

pub(crate) fn needs_evaluation(view: &SceneView, entity: Entity) -> bool {
    motion_in_chain(view, entity, 0)
}

pub(crate) fn has_changed_ancestor(view: &SceneView, entity: Entity, changed: &HashSet<Entity>) -> bool {
    let mut ancestor = view.object(entity).and_then(|object| match object.detail().summary.parent {
        Parent::Resolved(parent) => Some(parent),
        Parent::Root | Parent::Unknown | Parent::Pending { .. } => None,
    });
    for _ in 0..MAX_PARENT_EDGES {
        let Some(entity) = ancestor else { return false };
        if changed.contains(&entity) {
            return true;
        }
        ancestor = view.object(entity).and_then(|object| match object.detail().summary.parent {
            Parent::Resolved(parent) => Some(parent),
            Parent::Root | Parent::Unknown | Parent::Pending { .. } => None,
        });
    }
    false
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
    regions: &mut Vec<kinematics::RegionBounds>,
) {
    placements.clear();
    future_placements.clear();
    regions.clear();
    regions.extend(view.regions().filter_map(|region| {
        region.summary().info.resolved_origin().map(|origin| kinematics::RegionBounds { region_id: region.summary().region.get(), origin })
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
    regions: &[kinematics::RegionBounds],
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
                current.pose = WorldPose {
                    position: linear_position_with_view(view, sample, state, now, regions),
                    rotation: state.local_rotation(sample, now),
                };
                next.pose = WorldPose {
                    position: linear_position_with_view(view, sample, state, future, regions),
                    rotation: state.local_rotation(sample, future),
                };
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

#[cfg(test)]
fn linear_position(sample: MotionSample, state: MotionState, now: Instant, regions: &[kinematics::RegionBounds]) -> DVec3 {
    linear_position_raw(sample, state, now, regions)
}

#[cfg(test)]
fn position_at_effective(sample: MotionSample, elapsed: f64) -> DVec3 {
    kinematics::position_at_effective(sample.transform.pose.position, sample.velocity, sample.acceleration, elapsed)
}

fn linear_position_with_view(
    view: &SceneView,
    sample: MotionSample,
    state: MotionState,
    now: Instant,
    regions: &[kinematics::RegionBounds],
) -> DVec3 {
    let position = linear_position_raw(sample, state, now, regions);
    if sample.velocity == Vec3::ZERO && sample.acceleration == Vec3::ZERO { position } else { clamp_motion_height(view, sample, position) }
}

fn linear_position_raw(sample: MotionSample, state: MotionState, now: Instant, regions: &[kinematics::RegionBounds]) -> DVec3 {
    if sample.is_attachment || (sample.velocity == Vec3::ZERO && sample.acceleration == Vec3::ZERO) {
        let start = sample.transform.pose.position;
        return start;
    }
    state.linear_position(sample, now, regions)
}

fn clamp_motion_height(view: &SceneView, sample: MotionSample, mut position: DVec3) -> DVec3 {
    let ground = view.terrain_height_at(position);
    if sample.kind == world::Kind::Avatar {
        if let Some(ground) = ground {
            position.z = position.z.max(ground + 0.5 * f64::from(sample.transform.scale.z));
        }
        return position;
    }
    let region_max_height = view
        .region(sample.region)
        .and_then(|region| region.summary().info.resolved_origin())
        .map_or(MAX_REGION_HEIGHT, |origin| origin.z + MAX_REGION_HEIGHT);
    position.z = position.z.min(region_max_height);
    if let Some(ground) = ground {
        position.z = position.z.max(ground - 0.5 * f64::from(sample.transform.scale.length()));
    }
    position
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
            kind: world::Kind::Object,
            region: Region::new(1),
            local_position: Vec3::ZERO,
            local_rotation: Quat::IDENTITY,
            velocity,
            acceleration,
            angular_velocity,
            is_physical,
            time_dilation: 1.0,
            ping_delay: None,
            motion_update_kind: MotionUpdateKind::Full,
            is_attachment: false,
            source: None,
        }
    }

    fn explicit_zero_omega_source() -> UpdateSource {
        let limits = WorldLimits {
            max_region_registrations: 1,
            max_entities: 1,
            max_entities_per_region: 1,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: 4,
            max_journal_changes: 8,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        };
        let region = Region::new(1);
        let mut world = World::new(limits);
        let _ = world.adopt_region(region).expect("region adoption");
        let mut observation = ObjectObservation::new(
            UpdateSource::new(region, 1, OrderEvidence::Ordered(1), 0),
            Uuid::from_u128(1),
            NonZeroU32::new(1).unwrap(),
            world::Kind::Object,
            ParentUpdate::Root,
        );
        observation.angular_velocity = Some(Vec3::ZERO);
        let _ = world.observe_object(observation).expect("object observation");
        world.object(world.entity(Uuid::from_u128(1)).expect("object entity")).unwrap().motion_source().unwrap()
    }

    fn child_scene(parent_scale: Option<Vec3>) -> (World, Reconciler, SceneView) {
        child_scene_with_motion(parent_scale, Vec3::X * 2.0, None)
    }

    fn child_scene_with_motion(
        parent_scale: Option<Vec3>,
        parent_velocity: Vec3,
        parent_angular_velocity: Option<Vec3>,
    ) -> (World, Reconciler, SceneView) {
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
        let terrain = world::TerrainPatch::new(1, 0, Box::new([0.0; world::TERRAIN_PATCH_SAMPLES])).expect("flat terrain patch");
        let _ = world.observe_terrain_patch(source(1), terrain).expect("terrain patch");
        let mut root =
            ObjectObservation::new(source(1), Uuid::from_u128(1), NonZeroU32::new(1).unwrap(), world::Kind::Object, ParentUpdate::Root);
        root.pose = Some(Pose { position: Some(Vec3::new(20.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        root.linear_motion = Some(LinearMotion { velocity: Some(parent_velocity), acceleration: None });
        root.angular_velocity = parent_angular_velocity;
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
        reconcile_scene(&mut scene, &mut view, &world, false);
        assert_eq!(view.object_count(), 2, "both linkset objects are published");
        (world, scene, view)
    }

    fn reconcile_scene(scene: &mut Reconciler, view: &mut SceneView, world: &World, notify: bool) {
        if notify {
            scene.notify_world_change();
        }
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
                ReconcileStep::Idle => return,
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
        assert_eq!(kinematics::effective_elapsed(1.0, 1.0), 1.0);
        close_vec(position, expected);
        assert_eq!(MotionState::new(now).elapsed(now), 0.0);
    }

    #[test]
    fn receipt_time_sample_uses_ping_and_frame_lead() {
        let received_at = Instant::now();
        let mut sample = sample(DVec3::ZERO, Vec3::X * 2.0, Vec3::ZERO, Vec3::ZERO, false);
        sample.time_dilation = 0.5;
        sample.ping_delay = Some(Duration::from_millis(100));
        sample.source =
            Some(UpdateSource::new(Region::new(1), 1, OrderEvidence::Ordered(1), 0).with_motion_timing(received_at, sample.ping_delay));
        let state = MotionState::new(received_at);

        let position = linear_position_raw(sample, state, received_at + Duration::from_secs(1), &[]);

        assert!((position.x - 1.058).abs() < 1e-12);
    }

    #[test]
    fn render_prediction_recovers_from_taper_position() {
        let sampled_at = Instant::now();
        let phase_out_at = sampled_at + Duration::from_secs_f64(kinematics::PHASE_OUT_TIME);
        let recovered_at = phase_out_at + Duration::from_millis(500);
        let sample = sample(DVec3::ZERO, Vec3::X, Vec3::ZERO, Vec3::ZERO, false);
        let mut state = MotionState::new(sampled_at);
        state.start_linear_phase_out(sample, Some(phase_out_at));
        let before_taper = linear_position_raw(sample, state, phase_out_at, &[]);
        let displayed = linear_position_raw(sample, state, recovered_at, &[]);

        state.resume_linear_at(sample, recovered_at, &[]);

        assert_eq!(before_taper, DVec3::X * kinematics::PHASE_OUT_TIME);
        assert_eq!(linear_position_raw(sample, state, recovered_at, &[]), displayed);
        assert_eq!(linear_position_raw(sample, state, recovered_at + Duration::from_millis(100), &[]), displayed + DVec3::X * 0.1);
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
        update.source = Some(explicit_zero_omega_source());
        let updated = MotionState::updated(state, moving, update, updated_at);

        close_rotation(updated.local_rotation(update, updated_at), update.local_rotation);
    }

    #[test]
    fn missing_compressed_omega_keeps_spin() {
        let received_at = Instant::now();
        let moving = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::Z, false);
        let state = MotionState::new(received_at);
        let updated_at = received_at + Duration::from_secs(1);
        let mut update = sample(DVec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, false);
        update.source = Some(UpdateSource::new(Region::new(1), 2, OrderEvidence::Ordered(2), 0));
        let updated = MotionState::updated(state, moving, update, updated_at);

        close_rotation(updated.local_rotation(update, updated_at), Quat::from_rotation_z(1.0));
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
            let (_world, _scene, view) = child_scene(scale);
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
    fn stopped_parent_repositions_linkset_child() {
        let (mut world, mut scene, mut view) = child_scene_with_motion(None, Vec3::ZERO, Some(Vec3::Z));
        let parent_id = Uuid::from_u128(1);
        let child_id = Uuid::from_u128(2);
        let parent_entity = view.object(world.entity(parent_id).expect("parent entity")).expect("parent object").detail().summary.entity;
        let child_entity = view.object(world.entity(child_id).expect("child entity")).expect("child object").detail().summary.entity;
        let previous = view.object(parent_entity).expect("spinning parent").clone();
        let now = previous.motion_state().sampled_at + Duration::from_secs(1);
        let mut placements = HashMap::new();
        let mut future_placements = HashMap::new();
        let mut regions = Vec::new();
        fill_placement_pair(&view, now, now, &[child_entity], &mut placements, &mut future_placements, &mut regions);
        let spinning_child = placements.get(&child_entity).expect("animated child placement").pose.position;
        assert!(spinning_child.distance(DVec3::new(23.0, 0.0, 0.0)) > 1.0);
        let version = view.version();

        let mut stopped = ObjectObservation::new(
            UpdateSource::new(Region::new(1), 3, OrderEvidence::Ordered(3), 0),
            parent_id,
            NonZeroU32::new(1).unwrap(),
            world::Kind::Object,
            ParentUpdate::Root,
        );
        stopped.pose = Some(Pose { position: Some(Vec3::new(20.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        stopped.linear_motion = Some(LinearMotion { velocity: Some(Vec3::ZERO), acceleration: Some(Vec3::ZERO) });
        stopped.angular_velocity = Some(Vec3::ZERO);
        let _ = world.observe_object(stopped).expect("stopped parent update");
        reconcile_scene(&mut scene, &mut view, &world, true);

        let parent = view.object(parent_entity).expect("updated parent");
        assert!(parent.motion_changed_from(&previous));
        assert!(has_changed_ancestor(&view, child_entity, &HashSet::from([parent_entity])));
        assert!(view.changes_since(&version).unwrap().any(|change| change.motion_changed.contains(&parent_entity)));
        let now = parent.motion_state().sampled_at + Duration::from_millis(16);
        fill_placement_pair(&view, now, now, &[child_entity], &mut placements, &mut future_placements, &mut regions);

        close_vec(placements.get(&child_entity).expect("final child placement").pose.position, DVec3::new(23.0, 0.0, 0.0));
    }

    #[test]
    fn motion_keeps_extrapolating_past_three_seconds() {
        let (_world, _scene, view) = child_scene(None);
        let root = view.objects().find(|object| object.detail().summary.id == Uuid::from_u128(1)).expect("root is published");
        let entity = root.detail().summary.entity;
        let now = root.motion_state().sampled_at + Duration::from_secs(5);
        let mut placements = HashMap::new();
        let mut future_placements = HashMap::new();
        let mut regions = Vec::new();

        fill_placement_pair(&view, now, now + Duration::from_millis(16), &[entity], &mut placements, &mut future_placements, &mut regions);

        close_vec(placements.get(&entity).expect("root placement").pose.position, DVec3::new(30.0, 0.0, 0.0));
    }

    #[test]
    fn falling_avatar_clamps_to_terrain_plus_half_height() {
        let (_world, _scene, view) = child_scene(None);
        let now = Instant::now();
        let mut falling = sample(DVec3::new(20.0, 1.0, -10.0), Vec3::NEG_Z * 20.0, Vec3::ZERO, Vec3::ZERO, false);
        falling.kind = world::Kind::Avatar;
        falling.transform.scale.z = 2.0;
        let state = MotionState::new(now);

        let position = linear_position_with_view(&view, falling, state, now + Duration::from_secs(1), &[]);

        assert_eq!(position.z, 1.0);
    }

    #[test]
    fn object_prediction_clamps_to_region_max_height() {
        let (_world, _scene, view) = child_scene(None);
        let now = Instant::now();
        let high = sample(DVec3::new(20.0, 1.0, 5000.0), Vec3::Z, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);

        let position = linear_position_with_view(&view, high, state, now + Duration::from_secs(1), &[]);

        assert_eq!(position.z, 4096.0);
    }

    #[test]
    fn angular_only_object_keeps_authoritative_height() {
        let (_world, _scene, view) = child_scene(None);
        let now = Instant::now();
        let spinning = sample(DVec3::new(20.0, 1.0, -10.0), Vec3::ZERO, Vec3::ZERO, Vec3::Z, false);
        let state = MotionState::new(now);

        let position = linear_position_with_view(&view, spinning, state, now + Duration::from_secs(1), &[]);

        assert_eq!(position.z, -10.0);
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
            kinematics::RegionBounds { region_id: Region::new(1).get(), origin: DVec3::ZERO },
            kinematics::RegionBounds { region_id: Region::new(2).get(), origin: DVec3::new(256.0, 0.0, 0.0) },
        ];
        close_vec(linear_position(moving, state, now + Duration::from_millis(1500), &regions), DVec3::new(356.0, 20.0, 0.0));
    }

    #[test]
    fn loaded_neighbor_still_clips_at_the_next_missing_region() {
        let now = Instant::now();
        let moving = sample(DVec3::new(255.0, 20.0, 0.0), Vec3::X * 300.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [
            kinematics::RegionBounds { region_id: Region::new(1).get(), origin: DVec3::ZERO },
            kinematics::RegionBounds { region_id: Region::new(2).get(), origin: DVec3::new(256.0, 0.0, 0.0) },
        ];

        close_vec(linear_position(moving, state, now + Duration::from_millis(900), &regions), DVec3::new(512.0, 20.0, 0.0));
    }

    #[test]
    fn sample_outside_assigned_region_starts_from_loaded_neighbor() {
        let now = Instant::now();
        let moving = sample(DVec3::new(300.0, 20.0, 0.0), Vec3::X * 300.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [
            kinematics::RegionBounds { region_id: Region::new(1).get(), origin: DVec3::ZERO },
            kinematics::RegionBounds { region_id: Region::new(2).get(), origin: DVec3::new(256.0, 0.0, 0.0) },
        ];

        close_vec(linear_position(moving, state, now + Duration::from_millis(900), &regions), DVec3::new(512.0, 20.0, 0.0));
    }

    #[test]
    fn missing_neighbor_clips_at_region_edge() {
        let now = Instant::now();
        let moving = sample(DVec3::new(255.0, 20.0, 0.0), Vec3::X * 100.0, Vec3::ZERO, Vec3::ZERO, false);
        let state = MotionState::new(now);
        let regions = [kinematics::RegionBounds { region_id: Region::new(1).get(), origin: DVec3::ZERO }];
        close_vec(linear_position(moving, state, now + Duration::from_millis(1500), &regions), DVec3::new(256.0, 20.0, 0.0));
    }
}
