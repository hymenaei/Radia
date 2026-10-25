use super::support::{
    AssetChange, AssetId, Budget, Duration, EnvironmentAssignment, EnvironmentProperty, EnvironmentPush, EnvironmentScope,
    EnvironmentSource, ExperienceId, Instant, ParcelLocalId, ReadFailure, ReconcileStep, Reconciler, Region, Uuid, WorldReadError,
    prepared_asset, settle, settle_materials, source, world,
};

#[test]
fn parcel_environment_precedes_region_and_inherit_falls_through() {
    let mut world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let region_assignment = assignment(
        EnvironmentSource::Inline(b"{'type':'sky','name':'region'}".as_slice().into()),
        1,
        Some(9000),
        Some(3000),
        Some([1000.0, 2000.0, 3000.0]),
    );
    let _ = world.observe_environment(target, source(1, 1), region_assignment).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).map(|value| value.description().name.as_str()), Some("region"));
    let parcel = ParcelLocalId::new(7).unwrap();
    let _ = world.observe_agent_parcel(source(1, 2), Some(parcel)).unwrap();
    let parcel_target = world.environment_target(region, EnvironmentScope::Parcel(parcel)).unwrap();
    let parcel_assignment = assignment(
        EnvironmentSource::Inline(b"{'type':'sky','name':'parcel'}".as_slice().into()),
        1,
        Some(7200),
        None,
        Some([90.0, 80.0, 70.0]),
    );
    let _ = world.observe_environment(parcel_target, source(1, 3), parcel_assignment).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.environment(region).unwrap();
    assert_eq!(environment.description().name, "parcel");
    assert_eq!(environment.day_length_seconds(), 7200);
    assert_eq!(environment.day_offset_seconds(), 3000);
    assert_eq!(environment.altitudes(), [1000.0, 2000.0, 3000.0]);
    let inherit = assignment(EnvironmentSource::Inherit, 2, None, None, None);
    let _ = world.observe_environment(parcel_target, source(1, 4), inherit).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.environment(region).unwrap();
    assert_eq!(environment.description().name, "region");
    assert_eq!(environment.day_length_seconds(), 9000);
    assert_eq!(environment.day_offset_seconds(), 3000);
    assert_eq!(environment.altitudes(), [1000.0, 2000.0, 3000.0]);
}

#[test]
fn asset_and_inline_assignments_publish_decoded_settings() {
    let mut world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let asset = AssetId::new(Uuid::from_u128(111));
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let _ = world.observe_environment(target, source(1, 1), assignment(EnvironmentSource::Asset(asset), 1, None, None, None)).unwrap();
    scene.notify_world_change();
    let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.into_iter().next().expect("asset request");
    let ready = prepared_asset(request.request, b"{'type':'water','name':'asset'}".to_vec());
    scene.complete_asset(request.ticket, ready);
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.environment(region).unwrap();
    assert_eq!(environment.description().name, "asset");
    std::assert_matches!(&environment.description().settings, assets::EnvironmentSettings::Water(_));
    let inline = assignment(EnvironmentSource::Inline(b"{'type':'sky','name':'inline'}".as_slice().into()), 2, None, None, None);
    let _ = world.observe_environment(target, source(1, 2), inline).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.environment(region).unwrap();
    assert_eq!(environment.description().name, "inline");
    std::assert_matches!(&environment.description().settings, assets::EnvironmentSettings::Sky(_));
}

#[test]
fn environment_identity_tracks_only_settings_and_schedule() {
    let mut world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let settings = b"{'type':'sky','name':'same'}";
    let _ = world
        .observe_environment(
            target,
            source(1, 1),
            assignment(EnvironmentSource::Inline(settings.as_slice().into()), 1, Some(3600), Some(600), None),
        )
        .unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let original = view.environment(region).unwrap().identity();
    let _ = world
        .observe_environment(
            target,
            source(1, 2),
            assignment(EnvironmentSource::Inline(settings.as_slice().into()), 2, Some(3600), Some(600), None),
        )
        .unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().identity(), original);
    let _ = world
        .observe_environment(
            target,
            source(1, 3),
            assignment(EnvironmentSource::Inline(settings.as_slice().into()), 3, Some(7200), Some(600), None),
        )
        .unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let rescheduled = view.environment(region).unwrap().identity();
    assert_ne!(rescheduled, original);
    let _ = world
        .observe_environment(
            target,
            source(1, 4),
            assignment(EnvironmentSource::Inline(b"{'type':'sky','name':'changed'}".as_slice().into()), 4, Some(7200), Some(600), None),
        )
        .unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_ne!(view.environment(region).unwrap().identity(), rescheduled);
}

#[test]
fn stale_asset_completion_is_ignored_and_previous_value_stays_until_replacement() {
    let mut world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let old = assignment(EnvironmentSource::Inline(b"{'type':'sky','name':'old'}".as_slice().into()), 1, None, None, None);
    let _ = world.observe_environment(target, source(1, 1), old).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    let asset_a = AssetId::new(Uuid::from_u128(112));
    let _ = world.observe_environment(target, source(1, 2), assignment(EnvironmentSource::Asset(asset_a), 2, None, None, None)).unwrap();
    scene.notify_world_change();
    let request_a =
        settle_materials(&mut scene, &mut view, &world, &mut now).requested.into_iter().next().expect("first replacement request");
    assert_eq!(view.environment(region).unwrap().description().name, "old");
    let asset_b = AssetId::new(Uuid::from_u128(113));
    let _ = world.observe_environment(target, source(1, 3), assignment(EnvironmentSource::Asset(asset_b), 3, None, None, None)).unwrap();
    scene.notify_world_change();
    let replacement = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(replacement.released.contains(&request_a.ticket));
    let request_b = replacement.requested.into_iter().next().expect("second replacement request");
    scene.complete_asset(request_a.ticket, prepared_asset(request_a.request, b"{'type':'sky','name':'stale'}".to_vec()));
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().description().name, "old");
    scene.complete_asset(request_b.ticket, prepared_asset(request_b.request, b"{'type':'sky','name':'current'}".to_vec()));
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().description().name, "current");
}

#[test]
fn parcel_failure_falls_back_to_region_then_none() {
    let mut world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let region_asset = AssetId::new(Uuid::from_u128(114));
    let _ =
        world.observe_environment(target, source(1, 1), assignment(EnvironmentSource::Asset(region_asset), 1, None, None, None)).unwrap();
    let parcel = ParcelLocalId::new(7).unwrap();
    let _ = world.observe_agent_parcel(source(1, 2), Some(parcel)).unwrap();
    let parcel_target = world.environment_target(region, EnvironmentScope::Parcel(parcel)).unwrap();
    let parcel_asset = AssetId::new(Uuid::from_u128(115));
    let _ = world
        .observe_environment(parcel_target, source(1, 3), assignment(EnvironmentSource::Asset(parcel_asset), 1, None, None, None))
        .unwrap();
    scene.notify_world_change();
    let parcel_request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.into_iter().next().expect("parcel request");
    scene.complete_asset(parcel_request.ticket, assets::Status::Failed { failure: assets::Failure::Malformed, retry_at: None });
    let fallback = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(fallback.released.contains(&parcel_request.ticket));
    let region_request = fallback.requested.into_iter().next().expect("region fallback request");
    scene.complete_asset(region_request.ticket, assets::Status::Failed { failure: assets::Failure::Malformed, retry_at: None });
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.environment(region).is_none());
}

#[test]
fn retirement_releases_environment_ticket_and_source_charge() {
    let mut world = world();
    let region = Region::new(1);
    let other_region = Region::new(2);
    let _ = world.adopt_region(other_region).unwrap();
    let _ = world.patch_region(other_region, ::world::RegionInfo::default()).unwrap();
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let asset = AssetId::new(Uuid::from_u128(116));
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let _ = world.observe_environment(target, source(1, 1), assignment(EnvironmentSource::Asset(asset), 1, None, None, None)).unwrap();
    scene.notify_world_change();
    let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.into_iter().next().expect("asset request");
    let charged = budget.charged_bytes();
    let _ = world.retire_region(region).unwrap();
    scene.notify_world_change();
    let retired = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(retired.released.contains(&request.ticket));
    assert!(view.environment(region).is_none());
    scene.complete_asset(request.ticket, prepared_asset(request.request, b"{'type':'sky','name':'stale'}".to_vec()));
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.environment(region).is_none());
    let baseline_budget = Budget::default();
    let (mut baseline_scene, mut baseline_view) = Reconciler::new(baseline_budget.clone());
    settle(&mut baseline_scene, &mut baseline_view, &world, &mut now);
    assert_eq!(budget.charged_bytes(), baseline_budget.charged_bytes());
    assert!(budget.charged_bytes() < charged);
}

#[test]
fn reset_and_close_release_pending_environment_assets() {
    let mut world = world();
    let region = Region::new(1);
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let asset = AssetId::new(Uuid::from_u128(117));
    let _ = world.observe_environment(target, source(1, 1), assignment(EnvironmentSource::Asset(asset), 1, None, None, None)).unwrap();
    scene.notify_world_change();
    let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.into_iter().next().expect("asset request");
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else { panic!("expected journal read") };
    scene.complete_read(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
    let ReconcileStep::Asset(AssetChange::Release(ticket)) = scene.poll(now) else { panic!("expected environment release") };
    assert_eq!(ticket, request.ticket);
    scene.complete_asset(request.ticket, prepared_asset(request.request, b"{'type':'sky','name':'stale'}".to_vec()));
    assert!(view.environment(region).is_none());

    let mut close_world = super::support::world();
    let asset = AssetId::new(Uuid::from_u128(118));
    let (mut closed_scene, mut closed_view) = Reconciler::new(Budget::default());
    settle(&mut closed_scene, &mut closed_view, &close_world, &mut now);
    let target = close_world.environment_target(region, EnvironmentScope::Region).unwrap();
    let _ =
        close_world.observe_environment(target, source(1, 2), assignment(EnvironmentSource::Asset(asset), 2, None, None, None)).unwrap();
    closed_scene.notify_world_change();
    let request =
        settle_materials(&mut closed_scene, &mut closed_view, &close_world, &mut now).requested.into_iter().next().expect("asset request");
    closed_scene.close();
    let ReconcileStep::Asset(AssetChange::Release(ticket)) = closed_scene.poll(now) else { panic!("expected environment release") };
    assert_eq!(ticket, request.ticket);
    closed_scene.complete_asset(request.ticket, prepared_asset(request.request, b"{'type':'sky','name':'stale'}".to_vec()));
    assert!(closed_view.environment(region).is_none());
    if let ReconcileStep::Closed = closed_scene.poll(now) {
    } else {
        panic!("expected closed scene")
    }
}

fn assignment(
    source: EnvironmentSource,
    version: u32,
    day_length_seconds: Option<u32>,
    day_offset_seconds: Option<i32>,
    altitudes: Option<[f32; 3]>,
) -> EnvironmentAssignment {
    EnvironmentAssignment { source, version: Some(version), day_length_seconds, day_offset_seconds, altitudes }
}

#[test]
fn experience_properties_keep_current_owner() {
    let world = world();
    let region = world.region_page(None, 1).unwrap().regions[0].region;
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let a = ExperienceId::new(Uuid::from_u128(10)).unwrap();
    let b = ExperienceId::new(Uuid::from_u128(20)).unwrap();
    for (experience, name, value) in [(a, "gamma", "r1.0"), (a, "blue_density", "[r0.1,r0.2,r0.3]"), (b, "gamma", "r2.0")] {
        scene
            .push_environment(
                region,
                EnvironmentPush::Partial {
                    experience,
                    sky: vec![EnvironmentProperty { name: name.into(), source: value.as_bytes().into() }],
                    water: Vec::new(),
                    transition: Duration::ZERO,
                },
            )
            .unwrap();
        settle(&mut scene, &mut view, &world, &mut now);
    }
    scene
        .push_environment(
            region,
            EnvironmentPush::Partial {
                experience: a,
                sky: Vec::new(),
                water: vec![EnvironmentProperty { name: "water_fog_density".into(), source: b"r0.5".as_slice().into() }],
                transition: Duration::ZERO,
            },
        )
        .unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky_overrides().unwrap().gamma, Some(2.0));
    assert_eq!(view.viewer_environment().unwrap().water_overrides().unwrap().fog_density, Some(0.5));
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(a), transition: Duration::from_secs(1) }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    let invalid = EnvironmentPush::Partial {
        experience: a,
        sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"r9.0".as_slice().into() }],
        water: vec![EnvironmentProperty { name: "".into(), source: b"r1.0".as_slice().into() }],
        transition: Duration::ZERO,
    };
    assert_eq!(scene.push_environment(region, invalid), Err(render::scene::EnvironmentPushError::Invalid));
    let malformed = EnvironmentPush::Partial {
        experience: a,
        sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"'bad'".as_slice().into() }],
        water: Vec::new(),
        transition: Duration::ZERO,
    };
    assert_eq!(scene.push_environment(region, malformed), Err(render::scene::EnvironmentPushError::Invalid));
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.viewer_environment().unwrap();
    assert_eq!(environment.sky_properties().len(), 1);
    assert_eq!(environment.sky_properties()["gamma"].experience(), b);
    assert_eq!(environment.sky_properties()["gamma"].source(), b"r2.0");
    assert_eq!(environment.sky_overrides().unwrap().gamma, Some(2.0));
    scene.notify_world_change();
    let ReconcileStep::Read(job) = scene.poll(now) else { panic!("expected journal read") };
    scene.complete_read(job, Err(ReadFailure::World(WorldReadError::ResyncRequired)), now);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky_properties()["gamma"].experience(), b, "World resync must preserve viewer overrides");
    let retained = view.viewer_environment().unwrap().clone();
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(b), transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.viewer_environment().unwrap().sky_properties().is_empty(), "clearing the winner must not resurrect overwritten values");
    scene.close();
    drop(scene);
    drop(view);
    assert!(budget.charged_bytes() > 0, "retained viewer snapshot must keep its admission");
    assert_eq!(retained.sky_properties()["gamma"].source(), b"r2.0");
    drop(retained);
    assert_eq!(budget.charged_bytes(), 0);
}

#[test]
fn experience_assets_order_and_clear() {
    let world = world();
    let region = world.region_page(None, 1).unwrap().regions[0].region;
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let a = ExperienceId::new(Uuid::from_u128(10)).unwrap();
    let b = ExperienceId::new(Uuid::from_u128(20)).unwrap();
    let mut requests = Vec::new();
    for experience in [a, b] {
        scene
            .push_environment(
                region,
                EnvironmentPush::Full { experience, asset: AssetId::new(experience.uuid()), transition: Duration::ZERO },
            )
            .unwrap();
        requests.extend(settle_materials(&mut scene, &mut view, &world, &mut now).requested);
    }
    let ready = prepared_asset(requests[0].request, b"{'type':'sky','name':'test'}".to_vec());
    scene.complete_asset(requests[1].ticket, ready.clone());
    settle(&mut scene, &mut view, &world, &mut now);
    assert!(view.viewer_environment().is_none(), "later resolution must wait for earlier push");
    scene.complete_asset(requests[0].ticket, ready.clone());
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky().unwrap().experience(), b);
    assert!(view.viewer_environment().unwrap().water().is_none());
    scene
        .push_environment(region, EnvironmentPush::Full { experience: a, asset: AssetId::new(a.uuid()), transition: Duration::ZERO })
        .unwrap();
    let pending = settle_materials(&mut scene, &mut view, &world, &mut now).requested.pop().unwrap();
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(a), transition: Duration::ZERO }).unwrap();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(work.released.contains(&pending.ticket));
    scene.complete_asset(pending.ticket, ready);
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky().unwrap().experience(), b, "late completion must not restore cleared experience");
}

#[test]
fn cleared_sky_restores_experience_day() {
    let world = world();
    let region = world.region_page(None, 1).unwrap().regions[0].region;
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    let a = ExperienceId::new(Uuid::from_u128(10)).unwrap();
    let b = ExperienceId::new(Uuid::from_u128(20)).unwrap();
    for (experience, source) in [
        (
            a,
            "{'type':'daycycle','frames':{'w':{'type':'water'},'s':{'type':'sky'}},'tracks':[[{'key_keyframe':r0,'key_name':'w'}],[{'key_keyframe':r0,'key_name':'s'}],[],[],[]]}",
        ),
        (b, "{'type':'sky'}"),
    ] {
        scene
            .push_environment(
                region,
                EnvironmentPush::Full { experience, asset: AssetId::new(experience.uuid()), transition: Duration::ZERO },
            )
            .unwrap();
        let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.pop().unwrap();
        scene.complete_asset(request.ticket, prepared_asset(request.request, source.as_bytes().to_vec()));
        settle_materials(&mut scene, &mut view, &world, &mut now);
    }
    let current = view.viewer_environment().unwrap();
    assert_eq!(current.sky().unwrap().experience(), b);
    assert_eq!(current.water().unwrap().experience(), a);
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(b), transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(
        view.viewer_environment().unwrap().sky().unwrap().experience(),
        a,
        "clearing fixed sky must reveal the active experience day"
    );
    scene
        .push_environment(region, EnvironmentPush::Full { experience: b, asset: AssetId::new(b.uuid()), transition: Duration::ZERO })
        .unwrap();
    let request = settle_materials(&mut scene, &mut view, &world, &mut now).requested.pop().unwrap();
    scene.complete_asset(request.ticket, assets::Status::Failed { failure: assets::Failure::Malformed, retry_at: None });
    settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.viewer_environment().unwrap().sky().unwrap().experience(), a, "failed push must retain the previous environment");
    scene.push_environment(region, EnvironmentPush::Clear { experience: None, transition: Duration::ZERO }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    let current = view.viewer_environment().unwrap();
    assert!(current.sky().is_none() && current.water().is_none());
}
