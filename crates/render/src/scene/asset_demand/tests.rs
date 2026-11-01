use super::*;
use crate::scene::RegionData;
use math::DVec3;
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
            AssetChange::Release(_) => None,
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
            AssetChange::Release(_) => None,
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
