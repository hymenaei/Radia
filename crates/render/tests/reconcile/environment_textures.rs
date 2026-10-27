use test_support::texture::GRAY_4096_J2C;

use super::support::{
    AssetChange, AssetId, Budget, Duration, EnvironmentAssignment, EnvironmentProperty, EnvironmentPush, EnvironmentScope,
    EnvironmentSource, ExperienceId, Instant, ReconcileStep, Reconciler, Region, RegionInfo, Request, Uuid, prepared_asset,
    settle_materials, source, world,
};

#[test]
fn environment_keyframes_demand_unique_texture_ids_with_per_environment_and_global_caps() {
    let mut world = world();
    for region in 2..=5 {
        let region = Region::new(region);
        let _ = world.adopt_region(region).unwrap();
        let _ = world.patch_region(region, RegionInfo::default()).unwrap();
    }
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let ids = [1, 2, 3, 4, 5].map(Region::new);
    for region in ids {
        let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
        let bytes = daycycle_textures(u128::from(region.get()) * 10_000);
        std::assert_matches!(assets::EnvironmentDescription::decode(bytes.as_bytes()), Ok(_));
        let assignment = assignment(EnvironmentSource::Inline(bytes.into_bytes().into()), 1);
        let _ = world.observe_environment(target, source(region.get(), 1), assignment).unwrap();
    }
    scene.notify_world_change();
    let work = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(ids.iter().filter(|region| view.environment(**region).is_some()).count(), ids.len());
    let unique: std::collections::BTreeSet<_> = work
        .environment_requested
        .iter()
        .filter_map(|request| match request.request {
            Request::Texture { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert!(work.environment_requested.len() <= 64, "environment demand stays within the global cap");
    assert_eq!(unique.len(), work.environment_requested.len(), "keyframes and regions share each requested asset ID once");
    for region in ids {
        let start = u128::from(region.get()) * 10_000;
        let frame_ids: std::collections::BTreeSet<_> = (1..=20).map(|index| AssetId::new(Uuid::from_u128(start + index))).collect();
        assert_eq!(unique.intersection(&frame_ids).count(), 11, "the per-environment cap leaves eleven unique sun textures after defaults");
    }
}

#[test]
fn replacement_and_region_retirement_retain_environment_texture_tickets_until_close() {
    let mut world = world();
    let region = Region::new(1);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let first_id = AssetId::new(Uuid::from_u128(201));
    let first = assignment(EnvironmentSource::Inline(sky_with_sun(first_id).into_bytes().into()), 1);
    let _ = world.observe_environment(target, source(1, 1), first).unwrap();
    scene.notify_world_change();
    let initial = settle_materials(&mut scene, &mut view, &world, &mut now);
    let first_request = initial
        .environment_requested
        .iter()
        .find(|request| matches!(request.request, Request::Texture { id, .. } if id == first_id))
        .copied()
        .expect("custom environment texture demand");
    scene.complete_asset(first_request.ticket, prepared_asset(first_request.request, GRAY_4096_J2C.to_vec()));
    let ready = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(ready.environment_requested.is_empty());
    assert_eq!(view.environment_texture(first_id).map(|texture| texture.asset()), Some(first_id));

    let second_id = AssetId::new(Uuid::from_u128(202));
    let second = assignment(EnvironmentSource::Inline(sky_with_sun(second_id).into_bytes().into()), 2);
    let _ = world.observe_environment(target, source(1, 2), second).unwrap();
    scene.notify_world_change();
    let replacement = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(!replacement.released.contains(&first_request.ticket));
    assert_eq!(view.environment_texture(first_id).map(|texture| texture.asset()), Some(first_id));
    let second_request = replacement
        .environment_requested
        .iter()
        .find(|request| matches!(request.request, Request::Texture { id, .. } if id == second_id))
        .copied()
        .expect("replacement environment texture demand");

    let _ = world.retire_region(region).unwrap();
    scene.notify_world_change();
    let retired = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert!(!retired.released.contains(&first_request.ticket));
    assert!(!retired.released.contains(&second_request.ticket));
    assert_eq!(view.environment_texture(first_id).map(|texture| texture.asset()), Some(first_id));

    scene.close();
    let mut released = std::collections::HashSet::new();
    for _ in 0..3 {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Release(ticket)) => {
                released.insert(ticket);
            }
            ReconcileStep::Closed => break,
            _ => {}
        }
    }
    assert!(released.contains(&first_request.ticket));
    assert!(released.contains(&second_request.ticket));
}

#[test]
fn close_releases_environment_texture_tickets() {
    let mut world = world();
    let region = Region::new(1);
    let target = world.environment_target(region, EnvironmentScope::Region).unwrap();
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    let id = AssetId::new(Uuid::from_u128(203));
    let assignment = assignment(EnvironmentSource::Inline(sky_with_sun(id).into_bytes().into()), 1);
    let _ = world.observe_environment(target, source(1, 1), assignment).unwrap();
    scene.notify_world_change();
    let pending = settle_materials(&mut scene, &mut view, &world, &mut now);
    let tickets: std::collections::HashSet<_> = pending.environment_requested.iter().map(|request| request.ticket).collect();
    assert!(!tickets.is_empty());
    scene.close();
    let mut released = std::collections::HashSet::new();
    for _ in 0..tickets.len() + 1 {
        match scene.poll(now) {
            ReconcileStep::Asset(AssetChange::Release(ticket)) => {
                released.insert(ticket);
            }
            ReconcileStep::Closed => break,
            _ => {}
        }
    }
    assert_eq!(released, tickets);
}

#[test]
fn experience_settings_and_overrides_share_texture_demands() {
    let world = world();
    let region = Region::new(1);
    let experience = ExperienceId::new(Uuid::from_u128(204)).unwrap();
    let asset = AssetId::new(experience.uuid());
    let setting_texture = AssetId::new(Uuid::from_u128(205));
    let sky_override_texture = AssetId::new(Uuid::from_u128(206));
    let water_override_texture = AssetId::new(Uuid::from_u128(207));
    let (mut scene, mut view) = Reconciler::new(Budget::default());
    let mut now = Instant::now();
    scene.push_environment(region, EnvironmentPush::Full { experience, asset, transition: Duration::ZERO }).unwrap();
    let request = settle_materials(&mut scene, &mut view, &world, &mut now)
        .requested
        .into_iter()
        .find(|request| request.request == Request::Environment(asset))
        .expect("experience environment request");
    scene.complete_asset(request.ticket, prepared_asset(request.request, sky_with_sun(setting_texture).into_bytes()));
    let setting = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(requested_id(&setting.environment_requested, setting_texture), 1);

    scene
        .push_environment(
            region,
            EnvironmentPush::Partial {
                experience,
                sky: vec![
                    EnvironmentProperty { name: "sun_id".into(), source: format!("u{}", setting_texture.uuid()).into_bytes().into() },
                    EnvironmentProperty {
                        name: "cloud_id".into(),
                        source: format!("u{}", sky_override_texture.uuid()).into_bytes().into(),
                    },
                ],
                water: Vec::new(),
                transition: Duration::ZERO,
            },
        )
        .unwrap();
    let sky_override = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(requested_id(&sky_override.environment_requested, setting_texture), 0);
    assert_eq!(requested_id(&sky_override.environment_requested, sky_override_texture), 1);

    scene
        .push_environment(
            region,
            EnvironmentPush::Partial {
                experience,
                sky: Vec::new(),
                water: vec![EnvironmentProperty {
                    name: "normal_map".into(),
                    source: format!("u{}", water_override_texture.uuid()).into_bytes().into(),
                }],
                transition: Duration::ZERO,
            },
        )
        .unwrap();
    let water_override = settle_materials(&mut scene, &mut view, &world, &mut now);
    assert_eq!(requested_id(&water_override.environment_requested, water_override_texture), 1);
}

fn requested_id(requests: &[super::support::AssetRequest], id: AssetId) -> usize {
    requests.iter().filter(|request| matches!(request.request, Request::Texture { id: requested, .. } if requested == id)).count()
}

fn sky_with_sun(id: AssetId) -> String {
    format!("{{'type':'sky','sun_id':u{}}}", id.uuid())
}

fn assignment(source: EnvironmentSource, version: u32) -> EnvironmentAssignment {
    EnvironmentAssignment { source, version: Some(version), day_length_seconds: None, day_offset_seconds: None, altitudes: None }
}

fn daycycle_textures(start: u128) -> String {
    let mut frames = String::new();
    let mut track = String::new();
    for index in 0_u32..20 {
        if index > 0 {
            frames.push(',');
            track.push(',');
        }
        let texture = Uuid::from_u128(start + u128::from(index) + 1);
        frames.push_str(&format!("'s{index}':{{'type':'sky','sun_id':u{texture}}}"));
        track.push_str(&format!("{{'key_keyframe':r{:.2},'key_name':'s{index}'}}", f64::from(index) / 20.0));
    }
    format!(
        "{{'type':'daycycle','frames':{{'water':{{'type':'water'}},{frames}}},'tracks':[[{{'key_keyframe':r0,'key_name':'water'}}],[{track}]]}}"
    )
}
