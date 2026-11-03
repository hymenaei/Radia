use std::assert_matches;

use super::*;
use crate::scene::RegionData;
use math::{DVec3, Vec3};
use world::{RegionInfo, RegionSummary, TerrainSettings};

fn settings(textures: [Uuid; 4]) -> TerrainSettings {
    TerrainSettings::new(textures, [0.0; 4], [64.0; 4]).unwrap()
}

fn region_data(budget: &Budget, region: Region, origin: DVec3, textures: [Uuid; 4]) -> RegionData {
    RegionData::new(
        RegionSummary {
            region,
            info: RegionInfo { origin: Some(origin), ..RegionInfo::default() },
            terrain_settings: Some(settings(textures)),
        },
        budget.reserve(512).unwrap(),
    )
}

fn camera() -> Camera {
    Camera::new(DVec3::new(128.0, 128.0, 32.0), DVec3::X.as_vec3(), 1.0, [1280, 720]).unwrap()
}

fn ids(seed: u128) -> [Uuid; 4] {
    std::array::from_fn(|index| Uuid::from_u128(seed + index as u128))
}

fn process(demand: &mut AssetDemand, mirror: &Mirror) {
    let _ = demand.step(mirror, &BTreeMap::new());
}

#[test]
fn live_region_texture_changes_release_old_demands_and_request_replacements() {
    let budget = Budget::default();
    let region = Region::new(1);
    let original = ids(10);
    let replacement = ids(20);
    let mut mirror = Mirror::default();
    mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, original));
    let mut demand = AssetDemand::new(budget.clone());
    demand.set_camera(camera());
    demand.invalidate_region(region);
    process(&mut demand, &mirror);
    let old_tickets: Vec<_> = std::iter::from_fn(|| demand.event())
        .filter_map(|change| match change {
            AssetChange::Request(request) => Some(request.ticket),
            AssetChange::Release(_) | AssetChange::UpdateImportance { .. } | AssetChange::UpdatePriority { .. } => None,
        })
        .collect();
    assert_eq!(old_tickets.len(), 4);
    assert_eq!(demand.terrain_targets[&region].textures, original);

    mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, replacement));
    demand.invalidate_region(region);
    process(&mut demand, &mirror);
    assert_eq!(demand.terrain_targets[&region].textures, replacement);
    let mut requested = 0;
    let mut released = 0;
    while let Some(change) = demand.event() {
        match change {
            AssetChange::Request(request) => {
                assert_eq!(request.region, region);
                requested += 1;
            }
            AssetChange::Release(_) => released += 1,
            AssetChange::UpdateImportance { .. } => panic!("replacement texture requests use a new ticket"),
            AssetChange::UpdatePriority { .. } => panic!("replacement texture requests use a new ticket"),
        }
    }
    assert_eq!(requested, 4);
    assert_eq!(released, 4);
    assert!(demand.entries.keys().all(|key| key.region != region || replacement.contains(&key.resource.id.uuid())));
}

#[test]
fn missing_and_failed_details_keep_the_neutral_fallback_for_each_layer() {
    let budget = Budget::default();
    let region = Region::new(1);
    let textures = ids(30);
    let mut mirror = Mirror::default();
    mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, textures));
    let mut demand = AssetDemand::new(budget);
    demand.set_camera(camera());
    demand.invalidate_region(region);
    process(&mut demand, &mirror);
    assert!(demand.terrain_textures(region).iter().all(Option::is_none));
    let tickets: Vec<_> = std::iter::from_fn(|| demand.event())
        .filter_map(|change| match change {
            AssetChange::Request(request) => Some(request.ticket),
            AssetChange::Release(_) | AssetChange::UpdateImportance { .. } | AssetChange::UpdatePriority { .. } => None,
        })
        .collect();
    assert_eq!(tickets.len(), 4);
    for ticket in tickets {
        let users = demand.complete(ticket, Status::Failed { failure: Failure::NotFound, retry_at: None });
        assert_eq!(users, [AssetUser::Terrain(region)]);
    }
    assert!(demand.terrain_textures(region).iter().all(Option::is_none));
    assert_eq!(demand.take_terrain_changes(), [region]);
}

#[test]
fn neighboring_regions_keep_separate_detail_texture_sets() {
    let budget = Budget::default();
    let south = Region::new(1);
    let east = Region::new(2);
    let south_textures = ids(40);
    let east_textures = ids(50);
    let mut mirror = Mirror::default();
    mirror.regions.insert(south, region_data(&budget, south, DVec3::ZERO, south_textures));
    mirror.regions.insert(east, region_data(&budget, east, DVec3::new(256.0, 0.0, 0.0), east_textures));
    let mut demand = AssetDemand::new(budget);
    demand.set_camera(camera());
    demand.invalidate_region(south);
    demand.invalidate_region(east);
    process(&mut demand, &mirror);
    process(&mut demand, &mirror);
    assert_eq!(demand.terrain_targets[&south].textures, south_textures);
    assert_eq!(demand.terrain_targets[&east].textures, east_textures);
    assert_eq!(demand.entries.keys().filter(|key| key.region == south).count(), 4);
    assert_eq!(demand.entries.keys().filter(|key| key.region == east).count(), 4);
}

#[test]
fn nearby_terrain_details_refine_beyond_initial_size() {
    let budget = Budget::default();
    let region = Region::new(1);
    let mut mirror = Mirror::default();
    mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, ids(60)));
    let close = camera().with_quality(64.0, 1.0).unwrap();
    let mut demand = AssetDemand::new(budget);
    demand.set_camera(close);
    demand.invalidate_region(region);
    process(&mut demand, &mirror);
    let nearby_size = demand.terrain_targets[&region].size;
    assert!(nearby_size.edge() > 128);

    demand.set_camera(close.with_quality(512.0, 1.0).unwrap());
    process(&mut demand, &mirror);
    assert_eq!(demand.terrain_targets[&region].size, nearby_size);
}

#[test]
fn mesh_importance_keeps_its_ticket() {
    let budget = Budget::default();
    let region = Region::new(1);
    let mut world = world::World::new(world::WorldLimits {
        max_region_registrations: 1,
        max_entities: 1,
        max_entities_per_region: 1,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: world::WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: world::WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: 8,
        max_journal_changes: 16,
        max_region_name_bytes: 64,
        max_recovery_targets: 1,
    });
    let _adopted = world.adopt_region(region).expect("fixture region registers");
    let _origin =
        world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("fixture origin applies");
    let asset = AssetId::new(Uuid::from_u128(90));
    let mut observation = world::ObjectObservation::new(
        world::UpdateSource::new(region, 1, world::OrderEvidence::Ordered(1), 0),
        Uuid::from_u128(91),
        std::num::NonZeroU32::new(1).expect("fixture local ID"),
        world::Kind::Object,
        world::ParentUpdate::Root,
    );
    observation.pose = Some(world::Pose { position: Some(Vec3::new(10.0, 0.0, 0.0)), rotation: Some(math::Quat::IDENTITY) });
    observation.scale = Some(Vec3::ONE);
    observation.shape = world::FieldUpdate::Set(world::Shape::Mesh { asset_id: asset, flags: world::SculptFlags::default() });
    let _applied = world.observe_object(observation).expect("fixture mesh applies");
    let item = world.object_list_page(None, 1).expect("fixture object list").objects.into_iter().next().expect("fixture object");
    let detail = world.object_detail(item.entity).expect("fixture detail");
    let lease = budget.reserve(detail.owned_bytes() + 32).expect("fixture detail fits");
    let mut mirror = Mirror::default();
    mirror.apply(crate::scene::model::Change::Object(crate::scene::Object::new(detail, lease)));
    let identities = BTreeMap::from([(item.id, item.entity)]);

    let mut demand = AssetDemand::new(budget);
    let first_camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1280, 720]).expect("first camera");
    demand.set_camera(first_camera);
    let _ = demand.step(&mirror, &identities);
    let ticket = std::iter::from_fn(|| demand.event())
        .find_map(|change| match change {
            AssetChange::Request(request) if matches!(request.request, Request::MeshHeader(id) if id == asset) => Some(request.ticket),
            _ => None,
        })
        .expect("mesh header request");

    let next_camera = Camera::new(DVec3::new(5.0, 0.0, 0.0), Vec3::X, 1.0, [1280, 720]).expect("camera shift");
    assert_ne!(
        first_camera.mesh_importance(DVec3::new(10.0, 0.0, 0.0), Vec3::ONE),
        next_camera.mesh_importance(DVec3::new(10.0, 0.0, 0.0), Vec3::ONE)
    );
    demand.set_camera(next_camera);
    let _ = demand.step(&mirror, &identities);
    assert_matches!(demand.event(), Some(AssetChange::UpdateImportance { ticket: updated, .. }) if updated == ticket);
    assert!(demand.event().is_none(), "importance-only movement leaves the active demand alone");

    demand.set_camera(first_camera);
    let _ = demand.step(&mirror, &identities);
    assert_matches!(demand.event(), Some(AssetChange::UpdateImportance { ticket: updated, .. }) if updated == ticket);
    assert!(demand.event().is_none());
}
