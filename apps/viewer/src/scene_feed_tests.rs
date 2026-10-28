use std::{
    collections::HashMap,
    future::{pending, ready},
    num::NonZeroU32,
    time::Duration,
};

use futures::{Future, FutureExt, join};
use math::{DVec3, Quat, Vec3};
use render::scene::{EnvironmentProperty, EnvironmentPush, Status};
use utils::{AssetId, ExperienceId};
use uuid::Uuid;
use world::{
    Appearance, FaceAppearance, FieldUpdate, Kind, ObjectObservation, ObjectPatch, OrderEvidence::Ordered, ParentUpdate, Pose, Region,
    RegionInfo, Shape, SurfaceAppearance, TerrainPatch, UpdateSource, World, WorldLimits,
};

use crate::asset_coordinator::AssetCoordinator;
use crate::world_reader::{ReadRequest, WorldReadHost};

use super::*;

async fn select_read<T>(service: &mut WorldReadHost, intake: impl Future<Output = T>) -> Result<T, ReadRequest> {
    if let Some(request) = service.take_reserved() {
        return Err(request);
    }
    let result = {
        let intake = intake.fuse();
        let request = service.next_request().fuse();
        pin_mut!(intake, request);
        futures::select_biased! {
            input = intake => Ok(input),
            request = request => Err(request),
        }
    };
    if result.is_ok() {
        service.record_intake_turn();
    }
    result
}

fn world() -> World {
    World::new(WorldLimits {
        max_region_registrations: 2,
        max_entities: 16,
        max_entities_per_region: 16,
        max_tombstones_per_region: 16,
        max_tombstones: 16,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(16),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(16),
        max_journal_entries: 32,
        max_journal_changes: 128,
        max_region_name_bytes: 64,
        max_recovery_targets: 8,
    })
}

fn advance_scene(world: &World, scene: &mut Reconciler, sender: &mpsc::Sender<Batch>, feed: &mut SceneFeed) {
    scene.notify_world_change();
    let now = Instant::now();
    for _ in 0..1000 {
        match scene.poll(now) {
            ReconcileStep::Read(job) => {
                let reply = match job.query() {
                    ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                    ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                    ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                    ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                    ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                    ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                    ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                };
                scene.complete_read(job, reply.map_err(ReadFailure::World), now);
            }
            ReconcileStep::Publish(batch) => {
                sender.try_send(batch).unwrap();
                feed.drain().unwrap();
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Continue => {}
            ReconcileStep::Idle => return,
            _ => panic!("expected scene reconciliation"),
        }
    }
    panic!("scene did not settle");
}

fn motion_sample(received_at: Instant) -> AgentMotionSample {
    AgentMotionSample {
        region: Region::new(1),
        position: DVec3::new(10.0, 20.0, 30.0),
        scale: Vec3::ONE,
        velocity: Vec3::ZERO,
        acceleration: Vec3::ZERO,
        time_dilation: 1.0,
        ping_delay: None,
        parented: false,
        simulator_rotation: Quat::IDENTITY,
        received_at,
        seed: 1,
    }
}

#[test]
fn dead_reckoning_uses_constant_velocity() {
    let received_at = Instant::now();
    let mut sample = motion_sample(received_at);
    sample.velocity = Vec3::new(2.0, -1.0, 0.5);
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));

    assert!(
        dead_reckoning.pose(received_at + Duration::from_secs(2), &[]).unwrap().position.distance(DVec3::new(14.016, 17.992, 31.004))
            < 1e-12
    );
}

#[test]
fn dead_reckoning_uses_timestep_acceleration() {
    let received_at = Instant::now();
    let mut sample = motion_sample(received_at);
    sample.acceleration = Vec3::new(0.0, 9.0, 0.0);
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));

    let expected = 20.0 + 0.5 * 9.0 * (1.0 - 1.0 / 45.0);
    assert!((dead_reckoning.pose(received_at + Duration::from_secs(1), &[]).unwrap().position.y - expected).abs() < 1e-12);
}

#[test]
fn dead_reckoning_applies_half_ping_lead_with_time_dilation() {
    let received_at = Instant::now();
    let mut sample = motion_sample(received_at);
    sample.velocity = Vec3::X * 2.0;
    sample.time_dilation = 0.5;
    sample.ping_delay = Some(Duration::from_millis(100));
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));

    let position = dead_reckoning.pose(received_at + Duration::from_secs(1), &[]).unwrap().position;

    assert!((position.x - 11.058).abs() < 1e-12);
}

#[test]
fn dead_reckoning_stops_at_an_unloaded_region_edge() {
    let received_at = Instant::now();
    let mut sample = motion_sample(received_at);
    sample.position = DVec3::new(255.0, 20.0, 10.0);
    sample.velocity = Vec3::X * 100.0;
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));
    let regions = [kinematics::RegionBounds { region_id: Region::new(1).get(), origin: DVec3::ZERO }];

    let pose = dead_reckoning.pose(received_at + Duration::from_secs(2), &regions).unwrap();

    assert_eq!(pose.position.x, 256.0);
}

#[test]
fn new_sample_replaces_state_immediately() {
    let received_at = Instant::now();
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(motion_sample(received_at)));
    let next = AgentMotionSample { position: DVec3::new(400.0, 500.0, 40.0), seed: 2, ..motion_sample(received_at) };
    dead_reckoning.replace(Some(next));

    assert_eq!(dead_reckoning.pose(received_at, &[]).unwrap().position, next.position);
}

#[test]
fn dead_reckoning_tapers_after_silence() {
    let received_at = Instant::now();
    let phase_out_at = received_at + Duration::from_secs_f64(kinematics::PHASE_OUT_TIME);
    let phase_out_duration = Duration::from_secs_f64(kinematics::MAX_INTERPOLATION_TIME - kinematics::PHASE_OUT_TIME);
    let phase_out_end = phase_out_at + phase_out_duration;
    let mut sample = motion_sample(received_at);
    sample.velocity = Vec3::X;
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));
    dead_reckoning.start_phase_out(Some(phase_out_at));

    assert!((dead_reckoning.pose(phase_out_at + phase_out_duration / 2, &[]).unwrap().position.x - 12.383).abs() < 1e-12);
    assert!((dead_reckoning.pose(phase_out_end, &[]).unwrap().position.x - 12.508).abs() < 1e-12);
    assert!((dead_reckoning.pose(phase_out_end + phase_out_duration, &[]).unwrap().position.x - 12.508).abs() < 1e-12);
    assert!(!dead_reckoning.active(phase_out_end + Duration::from_nanos(1)));
}

#[test]
fn disconnect_tapers_agent_early() {
    let received_at = Instant::now();
    let disconnect_at = received_at + Duration::from_secs_f64(kinematics::PHASE_OUT_TIME / 4.0);
    let phase_out_duration = Duration::from_secs_f64(kinematics::MAX_INTERPOLATION_TIME - kinematics::PHASE_OUT_TIME);
    let phase_out_end = disconnect_at + phase_out_duration;
    let mut sample = motion_sample(received_at);
    sample.velocity = Vec3::X;
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));
    dead_reckoning.start_phase_out(Some(disconnect_at));

    let at_sample = dead_reckoning.pose(received_at, &[]).unwrap().position;
    let at_disconnect = dead_reckoning.pose(disconnect_at, &[]).unwrap().position;
    let at_end = dead_reckoning.pose(phase_out_end, &[]).unwrap().position;

    assert_eq!(at_disconnect, at_sample + Vec3::X.as_dvec3() * (disconnect_at - received_at).as_secs_f64());
    assert_eq!(at_end, at_disconnect + Vec3::X.as_dvec3() * (phase_out_duration.as_secs_f64() / 2.0));
    assert_eq!(dead_reckoning.pose(phase_out_end + Duration::from_nanos(1), &[]).unwrap().position, at_end);
    assert!(!dead_reckoning.active(phase_out_end));
}

#[test]
fn dead_reckoning_recovers_from_displayed_position() {
    let received_at = Instant::now();
    let phase_out_at = received_at + Duration::from_secs_f64(kinematics::PHASE_OUT_TIME);
    let phase_out_duration = Duration::from_secs_f64(kinematics::MAX_INTERPOLATION_TIME - kinematics::PHASE_OUT_TIME);
    let recovered_at = phase_out_at + phase_out_duration / 2;
    let mut sample = motion_sample(received_at);
    sample.velocity = Vec3::X;
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));
    dead_reckoning.start_phase_out(Some(phase_out_at));
    let displayed = dead_reckoning.pose(recovered_at, &[]).unwrap().position;

    dead_reckoning.resume_at(recovered_at, &[]);

    assert_eq!(dead_reckoning.pose(recovered_at, &[]).unwrap().position, displayed);
    assert_eq!(dead_reckoning.pose(recovered_at + Duration::from_millis(100), &[]).unwrap().position, displayed + DVec3::X * 0.1);
}

#[test]
fn negligible_motion_is_idle() {
    let received_at = Instant::now();
    let mut sample = motion_sample(received_at);
    sample.velocity = Vec3::splat(1e-4);
    sample.acceleration = Vec3::splat(1e-4);
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(sample));
    assert!(!dead_reckoning.active(received_at));
    sample.velocity = Vec3::X * 0.01;
    dead_reckoning.replace(Some(sample));
    assert!(dead_reckoning.active(received_at));
}

#[test]
fn non_finite_sample_does_not_replace_valid_state() {
    let received_at = Instant::now();
    let valid = motion_sample(received_at);
    let mut dead_reckoning = DeadReckoning::default();
    dead_reckoning.replace(Some(valid));
    let invalid = AgentMotionSample { velocity: Vec3::splat(f32::INFINITY), ..valid };
    dead_reckoning.replace(Some(invalid));

    assert_eq!(dead_reckoning.pose(received_at, &[]).unwrap().position, valid.position);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[tokio::test]
async fn motion_sample_seeds_follow_camera_before_the_avatar_resolves() {
    let agent_id = Uuid::from_u128(1);
    let now = Instant::now();
    let sample = AgentMotionSample {
        region: Region::new(1),
        position: DVec3::new(270.0, 520.0, 24.0),
        scale: Vec3::ONE,
        velocity: Vec3::X,
        acceleration: Vec3::ZERO,
        time_dilation: 1.0,
        ping_delay: None,
        parented: false,
        simulator_rotation: Quat::IDENTITY,
        received_at: now,
        seed: 1,
    };
    let budget = Budget::default();
    let assets = AssetCoordinator::new(Handle::current(), pending(), None);
    let (_, view) = Reconciler::new(budget.clone());
    let (sender, incoming) = mpsc::channel(1);
    let (cancel, _) = oneshot::channel();
    let (agent_motion_sender, agent_motion) = watch::channel(Some(sample));
    let mut feed = SceneFeed {
        runtime: Handle::current(),
        view,
        budget,
        assets: assets.handle(),
        agent_id,
        agent: None,
        water_height: None,
        camera: watch::channel(None).0,
        movement: watch::channel(None).0,
        agent_motion,
        motion_circuits: watch::channel(Vec::new()).1,
        motion_circuit_scratch: Vec::new(),
        motion_region_scratch: Vec::new(),
        motion_active: HashMap::new(),
        dead_reckoning: {
            let mut dead_reckoning = DeadReckoning::default();
            dead_reckoning.replace(Some(sample));
            dead_reckoning
        },
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        motion_source_seed: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        motion_source_seated: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        pose_seed: 0,
        sit_response: watch::channel(None).1,
        accepted_sit_response: None,
        last_movement: None,
        incoming,
        _cancel: cancel,
    };
    drop((sender, agent_motion_sender));

    assert!(feed.agent.is_none(), "the self avatar has not entered World yet");
    assert_eq!(
        feed.camera_pose_at(now),
        Some(sample.pose(sample.position + sample.velocity.as_dvec3() * 0.008)),
        "follow mode uses the sample until World resolves the avatar"
    );
}

#[tokio::test]
async fn terrain_clamp_respects_parenting() {
    let mut world = world();
    let agent = Uuid::from_u128(1);
    for region in [Region::new(1), Region::new(2)] {
        let _ = world.adopt_region(region).unwrap();
        let _ = world
            .patch_region(
                region,
                RegionInfo { origin: Some(DVec3::new((region.get() - 1) as f64 * 256.0, 0.0, 7.0)), ..RegionInfo::default() },
            )
            .unwrap();
    }
    for region in [Region::new(1), Region::new(2)] {
        let terrain_source = UpdateSource::new(region, 1, Ordered(1), 0);
        let terrain = TerrainPatch::new(0, 0, Box::new([5.0; world::TERRAIN_PATCH_SAMPLES])).unwrap();
        let _ = world.observe_terrain_patch(terrain_source, terrain).unwrap();
    }
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(Region::new(1), 1, Ordered(1), 0),
            agent,
            NonZeroU32::new(1).unwrap(),
            Kind::Avatar,
            ParentUpdate::Root,
        ))
        .unwrap();
    let _ = world.observe_water_height(UpdateSource::new(Region::new(2), 1, Ordered(1), 0), 31.0).unwrap();
    let budget = Budget::default();
    let (mut scene, view) = Reconciler::new(budget.clone());
    let (sender, incoming) = mpsc::channel(1);
    let (cancel, _) = oneshot::channel();
    let (camera, _) = watch::channel(None);
    let (_agent_motion_sender, agent_motion) = watch::channel(None);
    let assets = AssetCoordinator::new(Handle::current(), pending(), None);
    let mut feed = SceneFeed {
        runtime: Handle::current(),
        view,
        budget,
        assets: assets.handle(),
        agent_id: agent,
        agent: None,
        water_height: None,
        camera,
        movement: watch::channel(None).0,
        agent_motion,
        motion_circuits: watch::channel(Vec::new()).1,
        motion_circuit_scratch: Vec::new(),
        motion_region_scratch: Vec::new(),
        motion_active: HashMap::new(),
        dead_reckoning: DeadReckoning::default(),
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        motion_source_seed: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        motion_source_seated: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        pose_seed: 0,
        sit_response: watch::channel(None).1,
        accepted_sit_response: None,
        last_movement: None,
        incoming,
        _cancel: cancel,
    };
    let mut hydrate = |world: &World| {
        scene.notify_world_change();
        let now = Instant::now();
        for _ in 0..1000 {
            match scene.poll(now) {
                ReconcileStep::Read(job) => {
                    let reply = match job.query() {
                        ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                        ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                        ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                        ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                        ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                        ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                        ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                    };
                    scene.complete_read(job, reply.map_err(ReadFailure::World), now);
                }
                ReconcileStep::Publish(batch) => {
                    sender.try_send(batch).unwrap();
                    feed.drain().unwrap();
                }
                ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
                ReconcileStep::Continue => {}
                ReconcileStep::Idle => {
                    return (
                        feed.water_height,
                        feed.agent.and_then(|entity| feed.view.object(entity)).map(|object| object.detail().summary.entity),
                    );
                }
                _ => panic!("expected scene reconciliation"),
            }
        }
        panic!("scene did not settle");
    };
    let (height, original) = hydrate(&world);
    assert_eq!(height, None, "another region's water must not supply the water height");
    let original = original.expect("the observed avatar is available before its water height");
    let _ = world.observe_water_height(UpdateSource::new(Region::new(1), 2, Ordered(2), 0), 17.0).unwrap();
    assert_eq!(hydrate(&world), (Some(24.0), Some(original)));
    let _ = world.retire_region(Region::new(1)).unwrap();
    assert_eq!(hydrate(&world), (None, None), "retirement must remove both the avatar and previous water height");
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(Region::new(2), 3, Ordered(3), 0),
            agent,
            NonZeroU32::new(1).unwrap(),
            Kind::Avatar,
            ParentUpdate::Root,
        ))
        .unwrap();
    let (height, replacement) = hydrate(&world);
    assert_eq!(height, Some(38.0), "the new avatar region supplies its own observed height");
    assert_ne!(replacement.expect("the replacement avatar is available"), original, "a retired entity must not remain cached");
    drop(hydrate);
    let received_at = Instant::now();
    let mut falling_agent = motion_sample(received_at);
    falling_agent.region = Region::new(2);
    falling_agent.position = DVec3::new(266.0, 10.0, -10.0);
    falling_agent.velocity = Vec3::NEG_Z * 20.0;
    feed.dead_reckoning.replace(Some(falling_agent));
    let pose = feed.camera_pose_at(received_at + Duration::from_secs(1)).expect("self motion pose");
    assert_eq!(pose.position.z, 12.5);

    let mut seated_agent = motion_sample(received_at);
    seated_agent.region = Region::new(2);
    seated_agent.position = DVec3::new(266.0, 10.0, -10.0);
    seated_agent.parented = true;
    feed.dead_reckoning.replace(Some(seated_agent));
    let pose = feed.camera_pose_at(received_at).expect("seated self motion pose");
    assert_eq!(pose.position.z, -10.0);
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[tokio::test]
async fn pending_parent_does_not_block_flight() {
    use crate::agent_movement::HeldControls;

    let mut world = world();
    let region = Region::new(1);
    let agent_id = Uuid::from_u128(1);
    let seat_id = Uuid::from_u128(2);
    let agent_local_id = NonZeroU32::new(1).unwrap();
    let seat_local_id = NonZeroU32::new(2).unwrap();
    let _ = world.adopt_region(region).unwrap();
    let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            agent_id,
            agent_local_id,
            Kind::Avatar,
            ParentUpdate::Local(seat_local_id),
        ))
        .unwrap();

    let budget = Budget::default();
    let (mut scene, view) = Reconciler::new(budget.clone());
    let (sender, incoming) = mpsc::channel(1);
    let (cancel, _) = oneshot::channel();
    let (sit_response_sender, sit_response) = watch::channel(None);
    let assets = AssetCoordinator::new(Handle::current(), pending(), None);
    let mut feed = SceneFeed {
        runtime: Handle::current(),
        view,
        budget,
        assets: assets.handle(),
        agent_id,
        agent: None,
        water_height: None,
        camera: watch::channel(None).0,
        movement: watch::channel(None).0,
        agent_motion: watch::channel(None).1,
        motion_circuits: watch::channel(Vec::new()).1,
        motion_circuit_scratch: Vec::new(),
        motion_region_scratch: Vec::new(),
        motion_active: HashMap::new(),
        dead_reckoning: DeadReckoning::default(),
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        motion_source_seed: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        motion_source_seated: None,
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        pose_seed: 0,
        sit_response,
        accepted_sit_response: None,
        last_movement: None,
        incoming,
        _cancel: cancel,
    };

    advance_scene(&world, &mut scene, &sender, &mut feed);
    std::assert_matches!(feed.agent().expect("agent is observed").detail().summary.parent, Parent::Pending { .. });
    assert!(!feed.agent_is_seated());
    let mut controls = HeldControls::default();
    controls.toggle_flying(feed.agent_is_seated());
    assert_eq!(controls.observation(false, Quat::IDENTITY).control_flags, 0x0000_2000);

    let camera = SitCameraOverride { eye_offset: Vec3::new(2.0, 0.0, 1.0), at_offset: Vec3::new(0.0, 0.0, 1.0) };
    sit_response_sender.send_replace(Some(SitResponseState { object_id: seat_id, accepted: true, camera_override: Some(camera) }));
    feed.drain().unwrap();
    assert!(feed.agent_is_seated(), "an accepted response blocks flight until World confirms the parent");
    let mut seated_controls = HeldControls::default();
    assert!(!seated_controls.toggle_flying(feed.agent_is_seated()));
    assert_eq!(seated_controls.observation(false, Quat::IDENTITY).control_flags, 0);
    assert_eq!(feed.sit_camera_pose(), None, "an unresolved parent does not produce a seat camera pose");

    let mut seat = ObjectObservation::new(
        UpdateSource::new(region, 2, Ordered(2), 0),
        seat_id,
        seat_local_id,
        Kind::Object,
        ParentUpdate::Root,
    );
    seat.pose = Some(Pose { position: Some(Vec3::new(4.0, 5.0, 6.0)), rotation: Some(Quat::IDENTITY) });
    seat.scale = Some(Vec3::ONE);
    let _ = world.observe_object(seat).unwrap();
    advance_scene(&world, &mut scene, &sender, &mut feed);
    std::assert_matches!(feed.agent().expect("agent remains observed").detail().summary.parent, Parent::Resolved(_));
    assert!(feed.agent_is_seated());
    assert_eq!(feed.accepted_sit_response, None);
    assert_eq!(feed.sit_camera_pose(), Some((camera, WorldPose { position: DVec3::new(4.0, 5.0, 6.0), rotation: Quat::IDENTITY })));

    sit_response_sender.send_replace(Some(SitResponseState { object_id: seat_id, accepted: false, camera_override: Some(camera) }));
    feed.drain().unwrap();
    assert!(feed.agent_is_seated(), "World parent state remains authoritative after stand is requested");
    assert_eq!(feed.sit_camera_pose(), Some((camera, WorldPose { position: DVec3::new(4.0, 5.0, 6.0), rotation: Quat::IDENTITY })));

    let _ = world.remove_object(UpdateSource::new(region, 3, Ordered(3), 0), seat_local_id).unwrap();
    advance_scene(&world, &mut scene, &sender, &mut feed);
    std::assert_matches!(feed.agent().expect("agent remains observed").detail().summary.parent, Parent::Pending { .. });
    assert!(!feed.agent_is_seated(), "a removed seat returns the parent to unresolved without implying a sit");
    assert_eq!(feed.sit_camera_pose(), None, "the camera override ends when World unparents the avatar");
}

#[tokio::test]
async fn pending_asset_preserves_scene() {
    let mut world = world();
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).unwrap();
    let mut object = ObjectObservation::new(
        UpdateSource::new(region, 1, Ordered(1), 0),
        Uuid::from_u128(1),
        NonZeroU32::new(1).unwrap(),
        Kind::Object,
        ParentUpdate::Root,
    );
    object.pose = Some(Pose { position: Some(Vec3::new(10.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
    object.scale = Some(Vec3::ONE);
    object.shape = FieldUpdate::Set(Shape::Prim(Default::default()));
    object.appearance = FieldUpdate::Set(Appearance::Surface);
    object.surface_appearance = Some(
        SurfaceAppearance::new(
            FaceAppearance { texture: Some(AssetId::new(Uuid::from_u128(100))), color: [255, 0, 0, 255], ..FaceAppearance::default() },
            Vec::new(),
        )
        .unwrap(),
    );
    let _ = world.observe_object(object).unwrap();
    let mut reads = WorldReadHost::new(&world, Handle::current());
    let assets = AssetCoordinator::new(Handle::current(), pending(), None);
    let (_pushes, environments) = crate::session::EnvironmentInbox::channel();
    let (_agent_motion_sender, agent_motion) = watch::channel(None);
    let mut feed = SceneFeed::start(
        SceneSources { reader: reads.reader(), environments, assets: assets.handle(), motion_circuits: watch::channel(Vec::new()).1 },
        Uuid::from_u128(2),
        watch::channel(None).0,
        watch::channel(None).0,
        agent_motion,
        watch::channel(None).1,
        Handle::current(),
        || {},
    );
    feed.set_camera(Some(Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1000, 1000]).unwrap()));
    let receiving = async {
        loop {
            let batch = feed.incoming.recv().await.expect("scene remains active");
            feed.view.apply(batch).unwrap();
            if let Some(face) = feed.view.objects().next().and_then(|object| object.materials()).and_then(|faces| faces.first())
                && let Some(binding) = &face.base_color_texture
                && binding.state().resolution == render::material::Resolution::Pending
            {
                assert_eq!(face.base_color_factor(), [1.0, 0.0, 0.0, 1.0]);
                assert!(binding.image().is_none());
                assert_eq!(face.alpha, render::material::AlphaBehavior::Unknown);
                break;
            }
        }
    }
    .fuse();
    let serving = async {
        loop {
            let Err(request) = select_read(&mut reads, pending::<()>()).await else { unreachable!() };
            request.serve(&world);
        }
    }
    .fuse();
    pin_mut!(receiving, serving);
    futures::select_biased! {
        _ = receiving => {}
        _ = serving => unreachable!(),
    }
}

#[tokio::test]
async fn stalled_publication_preserves_intake() {
    let mut world = world();
    let region = Region::new(1);
    let _ = world.adopt_region(region).unwrap();
    let _ = world
        .observe_object(ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            Uuid::from_u128(1),
            NonZeroU32::new(1).unwrap(),
            Kind::Object,
            ParentUpdate::Root,
        ))
        .unwrap();
    let mut service = WorldReadHost::new(&world, Handle::current());
    let reader = service.reader();
    let assets = AssetCoordinator::new(Handle::current(), pending(), None);
    let (pushes, environments) = crate::session::EnvironmentInbox::channel();
    let (_agent_motion_sender, agent_motion) = watch::channel(None);
    let mut feed = SceneFeed::start(
        SceneSources { reader: reader.clone(), environments, assets: assets.handle(), motion_circuits: watch::channel(Vec::new()).1 },
        Uuid::from_u128(1),
        watch::channel(None).0,
        watch::channel(None).0,
        agent_motion,
        watch::channel(None).1,
        Handle::current(),
        || {},
    );
    let batch = {
        let receiving = async {
            loop {
                let batch = feed.incoming.recv().await.expect("live Scene publication");
                if batch.status() == Status::Ready {
                    break batch;
                }
                feed.view.apply(batch).unwrap();
            }
        }
        .fuse();
        let serving = async {
            loop {
                let Err(request) = select_read(&mut service, pending::<()>()).await else { unreachable!() };
                request.serve(&world);
            }
        }
        .fuse();
        pin_mut!(receiving, serving);
        futures::select_biased! {
            batch = receiving => batch,
            _ = serving => unreachable!(),
        }
    };
    assert_eq!(feed.view.status(), Status::Loading);
    let initial = world.revision();
    let (barrier, turns) = join!(reader.barrier(), async {
        let mut turns = 0;
        while turns < 1024 {
            match select_read(&mut service, ready(())).await {
                Ok(()) => {
                    turns += 1;
                    let mut patch = ObjectPatch::new(
                        UpdateSource::new(region, turns + 1, Ordered(u64::from(turns + 1)), 0),
                        NonZeroU32::new(1).unwrap(),
                    );
                    patch.pose = Some(Pose { position: Some(Vec3::new(turns as f32, 0.0, 0.0)), rotation: None });
                    let _ = world.patch_object(patch).unwrap();
                    service.publish(&world);
                }
                Err(request) => request.serve(&world),
            }
        }
        turns
    });
    assert_eq!(turns, 1024);
    assert!(barrier.unwrap().revision > initial);
    assert!(world.revision() > initial);
    let experience = ExperienceId::new(Uuid::from_u128(44)).unwrap();
    let push = EnvironmentPush::Partial {
        experience,
        sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"r1.4".as_slice().into() }],
        water: Vec::new(),
        transition: Duration::ZERO,
    };
    assert!(pushes.try_send(crate::session::EnvironmentCommand { region, push }).is_ok());
    feed.view.apply(batch).unwrap();
    assert_eq!(feed.view.object_count(), 1);
    {
        let receiving = async {
            loop {
                let batch = feed.incoming.recv().await.expect("live Scene publication");
                feed.view.apply(batch).unwrap();
                if let Some(environment) = feed.view.viewer_environment() {
                    assert_eq!(environment.sky_properties()["gamma"].experience(), experience);
                    break;
                }
            }
        }
        .fuse();
        let serving = async {
            loop {
                let Err(request) = select_read(&mut service, pending::<()>()).await else { unreachable!() };
                request.serve(&world);
            }
        }
        .fuse();
        {
            pin_mut!(receiving, serving);
            futures::select_biased! { _ = receiving => {}, _ = serving => unreachable!() }
        }
    }
    service.finish();
    assert!(feed.incoming.recv().await.is_none());
    assert_eq!(feed.view.status(), Status::Closed);
}
