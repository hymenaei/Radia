use super::support::*;

#[test]
fn environment_changes_follow_world() {
    let mut world = world();
    let region = Region::new(1);
    let budget = Budget::default();
    let (mut scene, mut view) = Reconciler::new(budget.clone());
    let mut now = Instant::now();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().region_assignment, None);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let assignment = EnvironmentAssignment {
        source: EnvironmentSource::Inline(vec![0; 2 * 1024 * 1024].into()),
        version: Some(1),
        day_length_seconds: Some(14_400),
        day_offset_seconds: Some(0),
        altitudes: Some([1000.0, 2000.0, 3000.0]),
    };
    let _ = world.observe_environment(target, source(1, 1), assignment.clone()).unwrap();
    scene.notify_world_change();
    let (_, builds) = settle_work(&mut scene, &mut view, &world, &mut now);
    assert_eq!(builds, 0, "assignment updates do not rebuild geometry");
    assert_eq!(view.environment(region).unwrap().region_assignment, Some(assignment));
    assert_eq!(view.revision(), Some(world.revision()));
    let parcel = ParcelLocalId::new(7).unwrap();
    let _ = world.observe_agent_parcel(source(1, 2), Some(parcel)).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region).unwrap().parcel.as_ref().unwrap().parcel, parcel);
    let _ = world.retire_region(region).unwrap();
    scene.notify_world_change();
    settle(&mut scene, &mut view, &world, &mut now);
    assert_eq!(view.environment(region), None);
    scene.close();
    drop(scene);
    drop(view);
    assert_eq!(budget.charged_bytes(), 0, "retirement and teardown release environment source admission");
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
    scene.push_environment(region, EnvironmentPush::Clear { experience: Some(a), transition: Duration::from_secs(1) }).unwrap();
    settle(&mut scene, &mut view, &world, &mut now);
    let invalid = EnvironmentPush::Partial {
        experience: a,
        sky: vec![EnvironmentProperty { name: "gamma".into(), source: b"r9.0".as_slice().into() }],
        water: vec![EnvironmentProperty { name: "".into(), source: b"r1.0".as_slice().into() }],
        transition: Duration::ZERO,
    };
    assert_eq!(scene.push_environment(region, invalid), Err(render::scene::EnvironmentPushError::Invalid));
    settle(&mut scene, &mut view, &world, &mut now);
    let environment = view.viewer_environment().unwrap();
    assert_eq!(environment.sky_properties().len(), 1);
    assert_eq!(environment.sky_properties()["gamma"].experience(), b);
    assert_eq!(environment.sky_properties()["gamma"].source(), b"r2.0");
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
