use std::time::UNIX_EPOCH;

use math::{DVec3, Quat, Vec3};
use utils::{AssetId, ExperienceId};
use uuid::Uuid;
use world::{
    EnvironmentAssignment, EnvironmentScope, EnvironmentSource, Kind, ObjectObservation, OrderEvidence::Ordered, ParcelLocalId,
    ParentUpdate, Pose, RegionInfo, UpdateSource, World, WorldLimits,
};

use super::*;
use crate::scene::{
    AssetChange, Budget, EnvironmentProperty, EnvironmentPush, ReadFailure, ReadQuery, ReadReply, ReconcileStep, Reconciler,
};

fn world() -> World {
    World::new(WorldLimits {
        max_region_registrations: 2,
        max_entities: 8,
        max_entities_per_region: 8,
        max_tombstones_per_region: 8,
        max_tombstones: 8,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(8),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(8),
        max_journal_entries: 16,
        max_journal_changes: 64,
        max_region_name_bytes: 64,
        max_recovery_targets: 4,
    })
}

#[test]
fn active_environment_texture_demand_includes_current_and_next_assets() {
    let sources = Sources {
        base: None,
        schedule: Schedule::default(),
        experience_day: None,
        experience_sky: None,
        experience_water: None,
        sky_override: None,
        water_override: None,
        local: None,
    };
    let mut evaluated = evaluate(&sources, 0.0, 0.0);
    evaluated.sky.sun.texture = TextureBlend { current: Some(asset(1)), next: Some(asset(2)), factor: 0.25 };
    evaluated.sky.moon.texture = TextureBlend { current: Some(asset(3)), next: Some(asset(4)), factor: 0.75 };
    evaluated.sky.clouds.texture = TextureBlend { current: Some(asset(5)), next: None, factor: 0.0 };
    evaluated.sky.bloom = TextureBlend { current: Some(asset(6)), next: Some(asset(7)), factor: 0.5 };
    evaluated.sky.rainbow = TextureBlend { current: None, next: Some(asset(8)), factor: 1.0 };
    evaluated.sky.halo = TextureBlend { current: Some(asset(9)), next: None, factor: 0.0 };
    evaluated.water.normal_map = TextureBlend { current: Some(asset(10)), next: Some(asset(11)), factor: 0.5 };
    let mut active = ActiveEnvironment { evaluated: Some(evaluated), ..Default::default() };

    assert_eq!(active.texture_assets(), (1..=11).map(asset).collect());
    active.evaluated = None;
    assert!(active.texture_assets().is_empty());
}

fn asset(value: u128) -> AssetId {
    AssetId::new(Uuid::from_u128(value))
}

fn settle(scene: &mut Reconciler, view: &mut SceneView, world: &World) {
    let mut now = Instant::now();
    for _ in 0..10_000 {
        match scene.poll(now) {
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => return,
            ReconcileStep::Wait(until) => now = until,
            ReconcileStep::Read(job) => {
                let result = match job.query() {
                    ReadQuery::Barrier => Ok(ReadReply::Barrier(world.read_barrier())),
                    ReadQuery::Objects(cursor) => world.object_list_page(*cursor, 256).map(ReadReply::Objects),
                    ReadQuery::Regions(cursor) => world.region_page(*cursor, 64).map(ReadReply::Regions),
                    ReadQuery::Details(entities) => world.object_detail_batch(entities).map(ReadReply::Details),
                    ReadQuery::Changes(cursor) => world.change_page(*cursor, 256).map(ReadReply::Changes),
                    ReadQuery::Terrain(region, cursor) => world.terrain_page(*region, *cursor, 8).map(ReadReply::Terrain),
                    ReadQuery::Environment(region) => Ok(ReadReply::Environment(world.environment(*region))),
                };
                scene.complete_read(job, result.map_err(ReadFailure::World), now);
            }
            ReconcileStep::Build(job) => scene.complete_mesh(job.run()),
            ReconcileStep::Closed => panic!("unexpected scene closure"),
        }
    }
    panic!("environment scene did not settle");
}

fn add_region_environment(world: &mut World, region: Region, bytes: &[u8], altitudes: Option<[f32; 3]>) {
    let _ = world.adopt_region(region).unwrap();
    let origin = DVec3::new((region.get() - 1) as f64 * 256.0, 0.0, if region.get() == 2 { 500.0 } else { 0.0 });
    let _ = world.patch_region(region, RegionInfo { origin: Some(origin), ..RegionInfo::default() }).unwrap();
    let target = world.environment_target(region, world::EnvironmentScope::Region).unwrap();
    let assignment = EnvironmentAssignment {
        source: EnvironmentSource::Inline(bytes.into()),
        version: Some(1),
        day_length_seconds: None,
        day_offset_seconds: None,
        altitudes,
    };
    let _ = world.observe_environment(target, UpdateSource::new(region, 1, Ordered(1), 0), assignment).unwrap();
}

fn replace_region_environment(world: &mut World, region: Region, bytes: &[u8]) {
    let target = world.environment_target(region, world::EnvironmentScope::Region).unwrap();
    let assignment = EnvironmentAssignment {
        source: EnvironmentSource::Inline(bytes.into()),
        version: Some(2),
        day_length_seconds: None,
        day_offset_seconds: None,
        altitudes: None,
    };
    let _ = world.observe_environment(target, UpdateSource::new(region, 2, Ordered(2), 0), assignment).unwrap();
}

fn set_parcel_environment(world: &mut World, region: Region, bytes: &[u8]) {
    let parcel = ParcelLocalId::new(7).unwrap();
    let _ = world.observe_agent_parcel(UpdateSource::new(region, 2, Ordered(2), 0), Some(parcel)).unwrap();
    let target = world.environment_target(region, EnvironmentScope::Parcel(parcel)).unwrap();
    let assignment = EnvironmentAssignment {
        source: EnvironmentSource::Inline(bytes.into()),
        version: Some(1),
        day_length_seconds: None,
        day_offset_seconds: None,
        altitudes: None,
    };
    let _ = world.observe_environment(target, UpdateSource::new(region, 2, Ordered(3), 0), assignment).unwrap();
}

fn ready_environment(asset: AssetId, bytes: &[u8]) -> assets::Status {
    let now = Instant::now();
    let mut store = assets::Store::new(assets::Budget::default());
    let route = store.register_route().unwrap();
    let demand = store.request(route, assets::Request::Environment(asset), assets::Priority::Visible, now).unwrap();
    let assets::LoadJob::Fetch(job) = store.next_job(now).unwrap() else { panic!("fresh environment request used cached bytes") };
    let input = job.complete(Ok(bytes.to_vec())).unwrap();
    let decode = store.start_decode(input).unwrap();
    store.complete(decode.run(), now);
    demand.status()
}

fn push_experience(
    scene: &mut Reconciler,
    view: &mut SceneView,
    world: &World,
    region: Region,
    experience: ExperienceId,
    transition: Duration,
    bytes: &[u8],
) {
    let asset = AssetId::new(experience.uuid());
    scene.push_environment(region, EnvironmentPush::Full { experience, asset, transition }).unwrap();
    let now = Instant::now();
    let request = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                break request;
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("experience request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected experience work"),
        }
    };
    scene.complete_asset(request.ticket, ready_environment(asset, bytes));
    settle(scene, view, world);
}

fn push_sky_property_at(
    scene: &mut Reconciler,
    region: Region,
    experience: ExperienceId,
    name: &str,
    source: &[u8],
    transition: Duration,
    now: Instant,
) {
    scene
        .push_environment_at(
            region,
            EnvironmentPush::Partial {
                experience,
                sky: vec![EnvironmentProperty { name: name.into(), source: source.into() }],
                water: Vec::new(),
                transition,
            },
            now,
        )
        .unwrap();
}

fn push_full_at(
    scene: &mut Reconciler,
    view: &mut SceneView,
    world: &World,
    region: Region,
    experience: ExperienceId,
    transition: Duration,
    started: Instant,
    bytes: &[u8],
) {
    let asset = AssetId::new(experience.uuid());
    scene.push_environment_at(region, EnvironmentPush::Full { experience, asset, transition }, started).unwrap();
    let now = Instant::now();
    let request = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                break request;
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("environment request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected environment work"),
        }
    };
    scene.complete_asset(request.ticket, ready_environment(asset, bytes));
    settle(scene, view, world);
}

fn default_evaluated() -> Evaluated {
    evaluate(
        &Sources {
            base: None,
            schedule: Schedule::default(),
            experience_day: None,
            experience_sky: None,
            experience_water: None,
            sky_override: None,
            water_override: None,
            local: None,
        },
        0.0,
        0.0,
    )
}

fn assert_color(actual: [f32; 3], from: [f32; 3], to: [f32; 3], factor: f32) {
    for index in 0..3 {
        let expected = from[index] + (to[index] - from[index]) * factor;
        assert!((actual[index] - expected).abs() < 0.00001, "channel {index}: {actual:?} != {expected}");
    }
}

fn assert_scalar(actual: f32, from: f32, to: f32, factor: f32) {
    let expected = from + (to - from) * factor;
    assert!((actual - expected).abs() < 0.00001, "{actual} != {expected}");
}

fn push_local(
    scene: &mut Reconciler,
    view: &mut SceneView,
    world: &World,
    region: Region,
    preset: crate::scene::EnvironmentPreset,
    bytes: &[u8],
) {
    let asset = preset.asset_id().expect("local sky preset has an asset");
    push_local_asset(scene, view, world, region, crate::scene::EnvironmentChoice::Local(LocalChoice::Preset(preset)), asset, bytes);
}

fn push_local_asset(
    scene: &mut Reconciler,
    view: &mut SceneView,
    world: &World,
    region: Region,
    choice: crate::scene::EnvironmentChoice,
    asset: AssetId,
    bytes: &[u8],
) {
    scene.set_local_environment(choice, region).unwrap();
    let now = Instant::now();
    let request = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                break request;
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("local preset request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected local preset work"),
        }
    };
    scene.complete_asset(request.ticket, ready_environment(asset, bytes));
    settle(scene, view, world);
}

#[test]
fn active_environment_uses_agent_region_and_restarts_identity_on_region_change() {
    let mut world = world();
    let first_region = Region::new(1);
    let second_region = Region::new(2);
    add_region_environment(&mut world, first_region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
    add_region_environment(
            &mut world,
            second_region,
            b"{'type':'daycycle','frames':{'w':{'type':'water'},'a':{'type':'sky','sunlight_color':[r1,r0,r0]},'b':{'type':'sky','sunlight_color':[r0,r1,r0]},'c':{'type':'sky','sunlight_color':[r1,r1,r0]},'d':{'type':'sky','sunlight_color':[r0,r0,r1]}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'a'}],[{'key_keyframe':r0,'key_name':'b'}],[{'key_keyframe':r0,'key_name':'c'}],[{'key_keyframe':r0,'key_name':'d'}]]}",
            Some([100.0, 200.0, 300.0]),
        );
    let mut agent = ObjectObservation::new(
        UpdateSource::new(second_region, 1, Ordered(2), 0),
        Uuid::from_u128(100),
        std::num::NonZeroU32::new(1).unwrap(),
        Kind::Avatar,
        ParentUpdate::Root,
    );
    agent.pose = Some(Pose { position: Some(Vec3::new(0.0, 0.0, 10.0)), rotation: Some(Quat::IDENTITY) });
    agent.scale = Some(Vec3::ONE);
    let _ = world.observe_object(agent).unwrap();
    assert!(world.environment(first_region).is_some());
    assert!(world.environment(second_region).is_some());
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);
    assert!(view.environment(first_region).is_some());
    assert!(
        view.environment(second_region).is_some(),
        "regions={} status={:?} origins={:?}",
        view.region_count(),
        view.status(),
        view.regions().map(|region| region.summary().info.resolved_origin()).collect::<Vec<_>>()
    );
    let mut active = ActiveEnvironment::default();
    let now = Instant::now();
    let wall_time = UNIX_EPOCH + Duration::from_secs(1);
    assert!(active.update(&view, Some((first_region, 510.0)), wall_time, now));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([1.0, 0.0, 0.0]));
    let first_identity = active.composition;
    active.update(&view, Some((second_region, 510.0)), wall_time, now + Duration::from_millis(1));
    assert_ne!(active.composition, first_identity);
    assert!(active.next_deadline().is_some());
    active.update(&view, Some((second_region, 510.0)), wall_time, now + Duration::from_secs(6));
    let evaluated = active.evaluated().expect("second region has an environment");
    assert_eq!(evaluated.sky_track, Some(4));
    assert_eq!(evaluated.sky.sunlight, [0.0, 0.0, 1.0]);
}

#[test]
fn experience_settings_keep_day_sky_and_water_precedence() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let geometry_version = view.geometry_version();
    let day = ExperienceId::new(Uuid::from_u128(1)).unwrap();
    let sky = ExperienceId::new(Uuid::from_u128(2)).unwrap();
    let water = ExperienceId::new(Uuid::from_u128(3)).unwrap();
    let day_settings = b"{'type':'daycycle','frames':{'w':{'type':'water','water_fog_density':r2},'s':{'type':'sky','sunlight_color':[r0,r1,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'s'}],[],[],[]]}";
    push_experience(&mut scene, &mut view, &world, region, day, Duration::from_secs(1), day_settings);
    push_experience(&mut scene, &mut view, &world, region, sky, Duration::from_secs(2), b"{'type':'sky','sunlight_color':[r1,r0,r0]}");
    push_experience(&mut scene, &mut view, &world, region, water, Duration::from_secs(3), b"{'type':'water','water_fog_density':r7}");
    let mut active = ActiveEnvironment::default();
    let now = Instant::now();
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
    let evaluated = active.evaluated().expect("experience settings are published");
    assert_eq!(evaluated.sky.sunlight, [1.0, 0.0, 0.0]);
    assert_eq!(evaluated.water.fog_density, 7.0);
    assert_eq!(view.viewer_environment().and_then(|viewer| viewer.transition_for_region(region)), Some(Duration::from_secs(3)));
    assert_ne!(active.composition.map(|identity| identity.experience_settings), Some([None; 3]));
    push_experience(&mut scene, &mut view, &world, region, sky, Duration::from_secs(2), b"{'type':'sky','sunlight_color':[r0,r0,r1]}");
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(1));
    assert_ne!(active.evaluated().map(|value| value.sky.sunlight), Some([1.0, 0.0, 0.0]));
    assert_eq!(
        view.geometry_version(),
        geometry_version,
        "experience and evaluated environment updates do not invalidate geometry preparation"
    );
}

#[test]
fn cycle_rechecks_intermediate_sunlight() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(
            &mut world,
            region,
            b"{'type':'daycycle','frames':{'water':{'type':'water'},'a':{'type':'sky','sunlight_color':[r1,r0,r0]},'b':{'type':'sky','sunlight_color':[r0,r0,r1]}},'tracks':[[{'key_keyframe':r0,'key_name':'water'}],[{'key_keyframe':r0.25,'key_name':'a'},{'key_keyframe':r0.75,'key_name':'b'}]]}",
            None,
        );
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    let start = Instant::now();
    let wall = UNIX_EPOCH + Duration::from_secs(3_600);
    assert!(active.update(&view, Some((region, 0.0)), wall, start));
    let initial = active.evaluated().expect("cycle is evaluated").sky.sunlight;
    assert_eq!(active.next_deadline(), Some(start + Duration::from_secs(1)));
    assert!(active.update(&view, Some((region, 0.0)), wall + Duration::from_millis(500), start + Duration::from_millis(500)));
    assert_ne!(active.evaluated().map(|value| value.sky.sunlight), Some(initial));
    assert!(active.update(&view, Some((region, 0.0)), wall + Duration::from_secs(1), start + Duration::from_secs(1)));
    assert_ne!(active.evaluated().map(|value| value.sky.sunlight), Some(initial));
    assert_eq!(active.next_deadline(), Some(start + Duration::from_secs(2)));
}

#[test]
fn foreign_clear_preserves_shared_transition() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    let now = Instant::now();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now);
    scene.push_environment(Region::new(2), EnvironmentPush::Clear { experience: None, transition: Duration::ZERO }).unwrap();
    replace_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}");
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(101));
    let color = active.evaluated().unwrap().sky.sunlight;
    assert!(color[0] > 0.9 && color[1] < 0.1, "{color:?}");
}

#[test]
fn clear_transition_scopes_to_clear() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    let start = Instant::now();
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, start));
    let experience = ExperienceId::new(Uuid::from_u128(987)).unwrap();
    push_experience(&mut scene, &mut view, &world, region, experience, Duration::ZERO, b"{'type':'sky','sunlight_color':[r0,r0,r1]}");
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, start + Duration::from_millis(1)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(experience), transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world);
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, start + Duration::from_millis(2)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([1.0, 0.0, 0.0]));
    replace_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}");
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, start + Duration::from_millis(3));
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, start + Duration::from_millis(103));
    let sunlight = active.evaluated().expect("shared change remains active").sky.sunlight;
    assert!(sunlight[0] > 0.9 && sunlight[1] < 0.1);
}

#[test]
fn local_preset_beats_shared_and_experience_and_clear_restores_experience() {
    let mut world = world();
    let region = Region::new(1);
    let second_region = Region::new(2);
    add_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}", None);
    add_region_environment(&mut world, second_region, b"{'type':'sky','sunlight_color':[r1,r0,r1]}", None);
    set_parcel_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r1,r1,r0]}");
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let parcel_sources = compose_sources(&view, region, view.viewer_environment(), None, false);
    assert_eq!(evaluate(&parcel_sources, 0.0, 0.0).sky.sunlight, [1.0, 1.0, 0.0]);
    let experience = ExperienceId::new(Uuid::from_u128(4)).unwrap();
    push_experience(
        &mut scene,
        &mut view,
        &world,
        region,
        experience,
        Duration::from_secs(2),
        b"{'type':'sky','sunlight_color':[r1,r0,r0]}",
    );
    let override_property = EnvironmentProperty { name: "sunlight_color".into(), source: b"[r0,r1,r0]".as_slice().into() };
    scene
        .push_environment(
            region,
            EnvironmentPush::Partial { experience, sky: vec![override_property], water: Vec::new(), transition: Duration::ZERO },
        )
        .unwrap();
    settle(&mut scene, &mut view, &world);
    let water_experience = ExperienceId::new(Uuid::from_u128(5)).unwrap();
    push_experience(
        &mut scene,
        &mut view,
        &world,
        region,
        water_experience,
        Duration::from_secs(2),
        b"{'type':'water','water_fog_density':r7}",
    );
    let now = Instant::now();
    let mut active = ActiveEnvironment::default();
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));
    push_local(
        &mut scene,
        &mut view,
        &world,
        region,
        crate::scene::EnvironmentPreset::Midday,
        b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
    );
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(7.0));
    assert_eq!(active.next_deadline(), None);
    assert_eq!(
        view.viewer_environment().map(|environment| environment.selected()),
        Some(&crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)))
    );

    let _ = world.observe_agent_parcel(UpdateSource::new(region, 2, Ordered(4), 0), None).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);
    assert!(!active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(2)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    assert!(!active.update(&view, Some((second_region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(3)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(7.0));
    assert!(!active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(4)));

    scene.set_local_environment(crate::scene::EnvironmentChoice::Shared, region).unwrap();
    settle(&mut scene, &mut view, &world);
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(5)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(6));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));
    assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(7.0));
    assert_eq!(view.viewer_environment().map(|environment| environment.selected()), Some(&crate::scene::EnvironmentChoice::Shared));
}

#[test]
fn completed_local_preset_uses_agent_region_and_updates_active_sky() {
    let mut world = world();
    let first_region = Region::new(1);
    let agent_region = Region::new(2);
    add_region_environment(&mut world, first_region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
    add_region_environment(&mut world, agent_region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    let now = Instant::now();
    assert!(active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));

    scene
        .set_local_environment(
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)),
            agent_region,
        )
        .unwrap();
    let request = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => break request,
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("local preset request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected local preset work"),
        }
    };
    assert_eq!(request.region, agent_region);
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 1.0, 0.0]));

    scene.complete_asset(
        request.ticket,
        ready_environment(
            crate::scene::EnvironmentPreset::Midday.asset_id().expect("known sky asset"),
            b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
        ),
    );
    settle(&mut scene, &mut view, &world);
    assert!(active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(2)));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
}

#[test]
fn crossing_retries_local_asset_in_agent_region() {
    let mut world = world();
    let first_region = Region::new(1);
    let agent_region = Region::new(2);
    add_region_environment(&mut world, first_region, b"{'type':'sky'}", None);
    add_region_environment(&mut world, agent_region, b"{'type':'sky'}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let choice = crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday));
    let asset = crate::scene::EnvironmentPreset::Midday.asset_id().expect("known sky asset");
    let now = Instant::now();
    scene.set_local_environment(choice.clone(), first_region).unwrap();
    let first = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                break request;
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("first local request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
        }
    };
    assert_eq!(first.region, first_region);

    scene.set_local_environment(choice, agent_region).unwrap();
    assert!(matches!(scene.poll(now), ReconcileStep::Asset(AssetChange::Release(ticket)) if ticket == first.ticket));
    let retry = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) if request.request == assets::Request::Environment(asset) => {
                break request;
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("local request was not retried after the crossing"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
        }
    };
    assert_eq!(retry.region, agent_region);
    assert_ne!(retry.ticket, first.ticket);
    scene.complete_asset(first.ticket, ready_environment(asset, b"{'type':'sky','sunlight_color':[r1,r0,r0]}"));
    scene.complete_asset(retry.ticket, ready_environment(asset, b"{'type':'sky','sunlight_color':[r0,r0,r1]}"));
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    assert!(active.update(&view, Some((agent_region, 0.0)), UNIX_EPOCH, now));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
}

#[test]
fn inventory_water_preserves_local_sky_and_daycycle_replaces_both() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(
            &mut world,
            region,
            b"{'type':'daycycle','frames':{'water':{'type':'water','water_fog_density':r3},'sky':{'type':'sky','sunlight_color':[r1,r0,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'water'}],[{'key_keyframe':r0,'key_name':'sky'}],[],[],[]]}",
            None,
        );
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let now = Instant::now();
    let mut active = ActiveEnvironment::default();
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
    let initial_water = active.evaluated().expect("shared environment").water.clone();

    let sky_asset = AssetId::new(Uuid::from_u128(20));
    push_local_asset(
        &mut scene,
        &mut view,
        &world,
        region,
        crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Inventory {
            owner_id: Uuid::from_u128(1),
            item_id: Uuid::from_u128(21),
            asset_id: sky_asset,
        }),
        sky_asset,
        b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
    );
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_millis(1));
    assert!(active.next_deadline().is_some());
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(6));
    let local_sky = active.evaluated().expect("local sky").sky.clone();
    assert_eq!(local_sky.sunlight, [0.0, 0.0, 1.0]);
    assert_eq!(active.evaluated().map(|value| &value.water), Some(&initial_water));

    let water_asset = AssetId::new(Uuid::from_u128(22));
    push_local_asset(
        &mut scene,
        &mut view,
        &world,
        region,
        crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Inventory {
            owner_id: Uuid::from_u128(1),
            item_id: Uuid::from_u128(23),
            asset_id: water_asset,
        }),
        water_asset,
        b"{'type':'water','water_fog_density':r9}",
    );
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(6) + Duration::from_millis(1));
    assert!(active.next_deadline().is_some());
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(12));
    assert_eq!(active.evaluated().map(|value| value.sky.sunlight), Some([0.0, 0.0, 1.0]));
    assert_eq!(active.evaluated().map(|value| value.water.fog_density), Some(9.0));

    let day_asset = AssetId::new(Uuid::from_u128(24));
    push_local_asset(
            &mut scene,
            &mut view,
            &world,
            region,
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Inventory {
                owner_id: Uuid::from_u128(1),
                item_id: Uuid::from_u128(25),
                asset_id: day_asset,
            }),
            day_asset,
            b"{'type':'daycycle','frames':{'water':{'type':'water','water_fog_density':r12},'sky':{'type':'sky','sunlight_color':[r0,r1,r0]}},'tracks':[[{'key_keyframe':r0,'key_name':'water'}],[{'key_keyframe':r0,'key_name':'sky'}],[],[],[]]}",
        );
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(12) + Duration::from_millis(1));
    assert!(active.next_deadline().is_some());
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(18));
    let daycycle = active.evaluated().expect("local day cycle");
    assert_eq!(daycycle.sky.sunlight, [0.0, 1.0, 0.0]);
    assert_eq!(daycycle.water.fog_density, 12.0);
}

#[test]
fn closing_scene_releases_local_asset_request() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky'}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    scene
        .set_local_environment(
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)),
            region,
        )
        .unwrap();
    let now = Instant::now();
    let request = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request))
                if request.request
                    == assets::Request::Environment(crate::scene::EnvironmentPreset::Midday.asset_id().expect("preset asset")) =>
            {
                break request;
            }
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("local preset request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
        }
    };
    scene.close();
    let released = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Release(ticket)) if ticket == request.ticket => break true,
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Closed => break false,
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Wait(_) | ReconcileStep::Idle | ReconcileStep::Read(_) | ReconcileStep::Build(_) => {
                panic!("closed scene did not release the local preset request")
            }
        }
    };
    assert!(released, "scene closure releases the outstanding preset ticket before returning Closed");
}

#[test]
fn failed_local_preset_publishes_a_typed_error() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky'}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    scene
        .set_local_environment(
            crate::scene::EnvironmentChoice::Local(crate::scene::LocalChoice::Preset(crate::scene::EnvironmentPreset::Midday)),
            region,
        )
        .unwrap();
    let now = Instant::now();
    let request = loop {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Request(request)) => break request,
            ReconcileStep::Asset(_) | ReconcileStep::Continue => {}
            ReconcileStep::Wait(until) => assert!(until <= now),
            ReconcileStep::Publish(batch) => view.apply(batch).unwrap(),
            ReconcileStep::Idle => panic!("local preset request was not published"),
            ReconcileStep::Read(_) | ReconcileStep::Build(_) | ReconcileStep::Closed => panic!("unexpected scene work"),
        }
    };
    scene.complete_asset(request.ticket, assets::Status::Failed { failure: assets::Failure::NotFound, retry_at: None });
    settle(&mut scene, &mut view, &world);

    let environment = view.viewer_environment().expect("failure outcome is published");
    assert_eq!(environment.selected(), &crate::scene::EnvironmentChoice::default());
    assert_eq!(environment.local_error(), Some(assets::Failure::NotFound));
    assert!(environment.local().is_none());
}

#[test]
fn cloud_scroll_follows_windlight_axis_directions() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky'}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    let now = Instant::now();
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now));
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH + Duration::from_secs(1), now + Duration::from_millis(16));
    let scroll = active.cloud_scroll();
    assert!((scroll[0] - 0.998).abs() < 0.000001);
    assert!((scroll[1] - 0.0001).abs() < 0.000001);
}

#[test]
fn water_phase_uses_elapsed_runtime_seconds() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'water'}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    let now = Instant::now();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now);
    assert_eq!(active.water_scroll(), [0.0; 4]);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, now + Duration::from_secs(2));
    assert_eq!(active.animation_time(), 2.0);
    assert_eq!(active.water_scroll(), [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn coalesced_properties_keep_independent_deadlines() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let second_started = started + Duration::from_secs(1);
    let long = ExperienceId::new(Uuid::from_u128(700)).unwrap();
    let short = ExperienceId::new(Uuid::from_u128(701)).unwrap();
    push_sky_property_at(&mut scene, region, long, "sunlight_color", b"[r1,r0,r0]", Duration::from_secs(10), started);
    push_sky_property_at(&mut scene, region, short, "moon_brightness", b"r0.8", Duration::from_secs(2), second_started);
    settle(&mut scene, &mut view, &world);

    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    assert!(active.update(&view, Some((region, 0.0)), UNIX_EPOCH, second_started));
    let evaluated = active.evaluated().expect("coalesced overrides are evaluated");
    assert_color(evaluated.sky.sunlight, defaults.sky.sunlight, [1.0, 0.0, 0.0], 0.1);
    assert_scalar(evaluated.sky.moon_brightness, defaults.sky.moon_brightness, 0.8, 0.0);

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(3));
    let evaluated = active.evaluated().expect("both property transitions remain active");
    assert_color(evaluated.sky.sunlight, defaults.sky.sunlight, [1.0, 0.0, 0.0], 0.3);
    assert_eq!(evaluated.sky.moon_brightness, 0.8);

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(10));
    assert_eq!(active.evaluated().expect("long transition completes").sky.sunlight, [1.0, 0.0, 0.0]);
}

#[test]
fn earlier_short_property_finishes_first() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let second_started = started + Duration::from_secs(1);
    let short = ExperienceId::new(Uuid::from_u128(702)).unwrap();
    let long = ExperienceId::new(Uuid::from_u128(703)).unwrap();
    push_sky_property_at(&mut scene, region, short, "moon_brightness", b"r0.8", Duration::from_secs(2), started);
    push_sky_property_at(&mut scene, region, long, "sunlight_color", b"[r0, r1, r0]", Duration::from_secs(10), second_started);
    settle(&mut scene, &mut view, &world);

    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, second_started);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(3));
    let evaluated = active.evaluated().expect("both transitions are evaluated");
    assert_eq!(evaluated.sky.moon_brightness, 0.8);
    assert_color(evaluated.sky.sunlight, defaults.sky.sunlight, [0.0, 1.0, 0.0], 0.2);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(11));
    assert_eq!(active.evaluated().expect("long transition completes").sky.sunlight, [0.0, 1.0, 0.0]);
}

#[test]
fn same_key_replacement_starts_at_current_value() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let replacement_started = started + Duration::from_secs(2);
    let first = ExperienceId::new(Uuid::from_u128(704)).unwrap();
    let second = ExperienceId::new(Uuid::from_u128(705)).unwrap();
    push_sky_property_at(&mut scene, region, first, "sunlight_color", b"[r1,r0,r0]", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);
    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);

    push_sky_property_at(&mut scene, region, second, "sunlight_color", b"[r0,r0,r1]", Duration::from_secs(4), replacement_started);
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, replacement_started);
    let at_replacement =
        std::array::from_fn(|index| defaults.sky.sunlight[index] + ([1.0, 0.0, 0.0][index] - defaults.sky.sunlight[index]) * 0.2);
    assert_eq!(active.evaluated().expect("replacement begins").sky.sunlight, at_replacement);

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(4));
    assert_color(active.evaluated().expect("replacement continues").sky.sunlight, at_replacement, [0.0, 0.0, 1.0], 0.5);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(6));
    assert_eq!(active.evaluated().expect("replacement completes").sky.sunlight, [0.0, 0.0, 1.0]);
}

#[test]
fn clearing_one_property_fades_without_interrupting_another() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let second_started = started + Duration::from_secs(1);
    let clear_started = started + Duration::from_secs(2);
    let first = ExperienceId::new(Uuid::from_u128(706)).unwrap();
    let second = ExperienceId::new(Uuid::from_u128(707)).unwrap();
    push_sky_property_at(&mut scene, region, first, "sunlight_color", b"[r1,r0,r0]", Duration::from_secs(10), started);
    push_sky_property_at(&mut scene, region, second, "moon_brightness", b"r0.8", Duration::from_secs(4), second_started);
    scene
        .push_environment_at(region, EnvironmentPush::Clear { experience: Some(first), transition: Duration::from_secs(2) }, clear_started)
        .unwrap();
    settle(&mut scene, &mut view, &world);

    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, clear_started);
    let evaluated = active.evaluated().expect("clear and property updates are coalesced");
    assert_color(evaluated.sky.sunlight, defaults.sky.sunlight, [1.0, 0.0, 0.0], 0.2);
    assert_scalar(evaluated.sky.moon_brightness, defaults.sky.moon_brightness, 0.8, 0.25);
    assert!(!view.viewer_environment().unwrap().sky_properties().contains_key("sunlight_color"));

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(3));
    let evaluated = active.evaluated().expect("removal fade and remaining transition continue");
    let removal_from =
        std::array::from_fn(|index| defaults.sky.sunlight[index] + ([1.0, 0.0, 0.0][index] - defaults.sky.sunlight[index]) * 0.2);
    assert_color(evaluated.sky.sunlight, removal_from, defaults.sky.sunlight, 0.5);
    assert_scalar(evaluated.sky.moon_brightness, defaults.sky.moon_brightness, 0.8, 0.5);

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(4));
    let evaluated = active.evaluated().expect("cleared property returns to the base");
    assert_eq!(evaluated.sky.sunlight, defaults.sky.sunlight);
    assert_scalar(evaluated.sky.moon_brightness, defaults.sky.moon_brightness, 0.8, 0.75);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(5));
    assert_eq!(active.evaluated().expect("remaining transition completes").sky.moon_brightness, 0.8);
}

#[test]
fn clearing_last_property_fades_to_defaults() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let clear_started = started + Duration::from_secs(5);
    let experience = ExperienceId::new(Uuid::from_u128(709)).unwrap();
    push_sky_property_at(&mut scene, region, experience, "sunlight_color", b"[r1,r0,r0]", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);

    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, clear_started);
    let at_clear =
        std::array::from_fn(|index| defaults.sky.sunlight[index] + ([1.0, 0.0, 0.0][index] - defaults.sky.sunlight[index]) * 0.5);
    assert_eq!(active.evaluated().expect("property has reached its midpoint").sky.sunlight, at_clear);

    scene
        .push_environment_at(
            region,
            EnvironmentPush::Clear { experience: Some(experience), transition: Duration::from_secs(4) },
            clear_started,
        )
        .unwrap();
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, clear_started);
    assert_eq!(active.evaluated().expect("last property begins fading").sky.sunlight, at_clear);

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(7));
    let fading = active.evaluated().expect("last property fades without another source");
    assert_color(fading.sky.sunlight, at_clear, defaults.sky.sunlight, 0.5);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(9));
    assert!(active.evaluated().is_none());
}

#[test]
fn clearing_replaced_owner_keeps_current_property() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let replacement_started = started + Duration::from_secs(2);
    let clear_started = started + Duration::from_secs(3);
    let first = ExperienceId::new(Uuid::from_u128(708)).unwrap();
    let second = ExperienceId::new(Uuid::from_u128(709)).unwrap();
    push_sky_property_at(&mut scene, region, first, "sunlight_color", b"[r1,r0,r0]", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);
    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);

    push_sky_property_at(&mut scene, region, second, "sunlight_color", b"[r0,r0,r1]", Duration::from_secs(4), replacement_started);
    settle(&mut scene, &mut view, &world);
    scene
        .push_environment_at(region, EnvironmentPush::Clear { experience: Some(first), transition: Duration::from_secs(2) }, clear_started)
        .unwrap();
    settle(&mut scene, &mut view, &world);
    assert_eq!(view.viewer_environment().unwrap().sky_properties().get("sunlight_color").unwrap().experience(), second);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, clear_started);
    let from = std::array::from_fn(|index| defaults.sky.sunlight[index] + ([1.0, 0.0, 0.0][index] - defaults.sky.sunlight[index]) * 0.2);
    assert_color(active.evaluated().unwrap().sky.sunlight, from, [0.0, 0.0, 1.0], 0.25);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(6));
    assert_eq!(active.evaluated().unwrap().sky.sunlight, [0.0, 0.0, 1.0]);
}

#[test]
fn unrelated_region_update_does_not_restart_composition() {
    let world = world();
    let first_region = Region::new(1);
    let second_region = Region::new(2);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let first = ExperienceId::new(Uuid::from_u128(710)).unwrap();
    let second = ExperienceId::new(Uuid::from_u128(711)).unwrap();
    push_full_at(
        &mut scene,
        &mut view,
        &world,
        first_region,
        first,
        Duration::ZERO,
        started,
        b"{'type':'sky','sunlight_color':[r0,r1,r0]}",
    );
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((first_region, 0.0)), UNIX_EPOCH, started);
    push_full_at(
        &mut scene,
        &mut view,
        &world,
        first_region,
        second,
        Duration::from_secs(10),
        started,
        b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
    );
    active.update(&view, Some((first_region, 0.0)), UNIX_EPOCH, started);
    push_sky_property_at(
        &mut scene,
        second_region,
        ExperienceId::new(Uuid::from_u128(712)).unwrap(),
        "moon_brightness",
        b"r0.4",
        Duration::from_secs(2),
        started + Duration::from_secs(1),
    );
    active.update(&view, Some((first_region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(1));
    active.update(&view, Some((first_region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(3));
    assert_color(active.evaluated().unwrap().sky.sunlight, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 0.3);
}

#[test]
fn region_teardown_keeps_original_property_deadline() {
    let world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    settle(&mut scene, &mut view, &world);
    let started = Instant::now();
    let owner = ExperienceId::new(Uuid::from_u128(713)).unwrap();
    push_sky_property_at(&mut scene, region, owner, "sunlight_color", b"[r1,r0,r0]", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);
    let defaults = default_evaluated();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(3));
    assert_color(active.evaluated().unwrap().sky.sunlight, defaults.sky.sunlight, [1.0, 0.0, 0.0], 0.3);

    active.update(&view, None, UNIX_EPOCH, started + Duration::from_secs(4));
    assert!(active.evaluated().is_none());
    assert!(active.next_deadline().is_none());
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(5));
    assert_color(active.evaluated().unwrap().sky.sunlight, defaults.sky.sunlight, [1.0, 0.0, 0.0], 0.5);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(11));
    assert_eq!(active.evaluated().unwrap().sky.sunlight, [1.0, 0.0, 0.0]);
}

#[test]
fn property_finishes_during_base_transition() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r1,r0,r0]}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);

    let started = Instant::now();
    let first = ExperienceId::new(Uuid::from_u128(714)).unwrap();
    let second = ExperienceId::new(Uuid::from_u128(715)).unwrap();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    push_sky_property_at(&mut scene, region, first, "moon_brightness", b"r0.2", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(1));

    let replacement_started = started + Duration::from_secs(1);
    push_sky_property_at(&mut scene, region, second, "moon_brightness", b"r0.8", Duration::from_secs(2), replacement_started);
    replace_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}");
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, replacement_started);

    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(3));
    assert_eq!(active.evaluated().unwrap().sky.moon_brightness, 0.8);
    assert!(active.next_deadline().is_some(), "the five-second base transition is still active");
}

#[test]
fn property_finishes_during_altitude_transition() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(
            &mut world,
            region,
            b"{'type':'daycycle','frames':{'water':{'type':'water'},'low':{'type':'sky','sunlight_color':[r1,r0,r0]},'high':{'type':'sky','sunlight_color':[r0,r0,r1]}},'tracks':[[{'key_keyframe':r0,'key_name':'water'}],[{'key_keyframe':r0,'key_name':'low'}],[{'key_keyframe':r0,'key_name':'high'}]]}",
            Some([0.0, 100.0, 200.0]),
        );
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);

    let started = Instant::now();
    let region_owner = ExperienceId::new(Uuid::from_u128(716)).unwrap();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    let property_started = started + Duration::from_secs(1);
    push_sky_property_at(&mut scene, region, region_owner, "moon_brightness", b"r0.8", Duration::from_secs(2), property_started);
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 50.0)), UNIX_EPOCH, property_started);
    active.update(&view, Some((region, 50.0)), UNIX_EPOCH, started + Duration::from_secs(3));

    assert_eq!(active.evaluated().unwrap().sky.moon_brightness, 0.8);
    assert!(active.next_deadline().is_some(), "the altitude transition is still active");
}

#[test]
fn leaving_local_resumes_property_timeline() {
    let mut world = world();
    let region = Region::new(1);
    add_region_environment(&mut world, region, b"{'type':'sky','sunlight_color':[r0,r1,r0]}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);

    let started = Instant::now();
    let owner = ExperienceId::new(Uuid::from_u128(718)).unwrap();
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    push_sky_property_at(&mut scene, region, owner, "moon_brightness", b"r0.9", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started);
    let initial = active.evaluated().unwrap().sky.moon_brightness;

    push_local(
        &mut scene,
        &mut view,
        &world,
        region,
        crate::scene::EnvironmentPreset::Midday,
        b"{'type':'sky','sunlight_color':[r0,r0,r1]}",
    );
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_millis(1));
    assert_ne!(active.evaluated().unwrap().sky.moon_brightness, 0.9);

    scene.set_local_environment(crate::scene::EnvironmentChoice::Shared, region).unwrap();
    settle(&mut scene, &mut view, &world);
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_millis(5));
    active.update(&view, Some((region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(6));
    assert_scalar(active.evaluated().unwrap().sky.moon_brightness, initial, 0.9, 0.6);
}

#[test]
fn inactive_region_property_has_no_deadline() {
    let mut world = world();
    let first_region = Region::new(1);
    let second_region = Region::new(2);
    add_region_environment(&mut world, first_region, b"{'type':'sky'}", None);
    add_region_environment(&mut world, second_region, b"{'type':'sky','sunlight_color':[r0,r0,r1]}", None);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world);

    let started = Instant::now();
    let owner = ExperienceId::new(Uuid::from_u128(717)).unwrap();
    push_sky_property_at(&mut scene, first_region, owner, "moon_brightness", b"r0.8", Duration::from_secs(10), started);
    settle(&mut scene, &mut view, &world);
    let mut active = ActiveEnvironment::default();
    active.update(&view, Some((first_region, 0.0)), UNIX_EPOCH, started);
    let initial = active.evaluated().unwrap().sky.moon_brightness;

    active.update(&view, Some((second_region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(1));
    active.update(&view, Some((second_region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(6));
    assert_eq!(active.next_deadline(), None);

    active.update(&view, Some((first_region, 0.0)), UNIX_EPOCH, started + Duration::from_secs(7));
    assert_scalar(active.evaluated().unwrap().sky.moon_brightness, initial, 0.8, 0.7);
}
