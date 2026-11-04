use std::assert_matches;

use super::*;
use crate::scene::RegionData;
use math::{DVec3, Vec3};
use world::{OrderEvidence, RegionInfo, RegionSummary, TerrainSettings, World, WorldLimits};

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

fn ready_header_status(asset: AssetId) -> Status {
    let now = std::time::Instant::now();
    let mut store = assets::Store::new(assets::Budget::default());
    let route = store.register_route().expect("fixture route registers");
    let demand = store.request(route, Request::MeshHeader(asset), Priority::Visible, now).expect("fixture header request registers");
    let Some(assets::LoadJob::Fetch(job)) = store.next_job(now) else { panic!("fixture header fetch starts") };
    let input = job.complete(Ok(test_support::mesh::asset(&[0, 1, 2, 3], 1))).expect("fixture header response fits");
    let completion = store.start_decode(input).expect("fixture header decode starts").run();
    assert!(store.complete(completion, now).is_empty());
    demand.status()
}

fn invalidate_completed(demand: &mut AssetDemand, mirror: &Mirror, ticket: AssetTicket, status: Status) -> usize {
    let users = demand.complete(ticket, status);
    let count = users.len();
    for user in users {
        let AssetUser::Object(entity) = user else { panic!("mesh header users are objects") };
        demand.invalidate(mirror.objects.get(&entity).expect("completed mesh user remains in the scene"));
    }
    count
}

fn mesh_scene(count: u32) -> (Budget, Mirror, BTreeMap<Uuid, Entity>) {
    let budget = Budget::default();
    let region = Region::new(1);
    let object_count = usize::try_from(count).expect("fixture object count fits usize");
    let tombstone_bytes = WorldLimits::tombstone_bytes_for_count(object_count);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: object_count,
        max_entities_per_region: object_count,
        max_tombstones_per_region: object_count,
        max_tombstones: object_count,
        max_tombstone_bytes_per_region: tombstone_bytes,
        max_tombstone_bytes: tombstone_bytes,
        max_journal_entries: 4096,
        max_journal_changes: 8192,
        max_region_name_bytes: 64,
        max_recovery_targets: 1,
    });
    let _adopted = world.adopt_region(region).expect("fixture region registers");
    let _origin =
        world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("fixture origin applies");

    for local_id in 1..=count {
        let asset = AssetId::new(Uuid::from_u128(10_000 + u128::from(local_id)));
        let mut observation = world::ObjectObservation::new(
            world::UpdateSource::new(region, local_id, OrderEvidence::Ordered(u64::from(local_id)), 0),
            Uuid::from_u128(u128::from(local_id)),
            std::num::NonZeroU32::new(local_id).expect("fixture local ID is nonzero"),
            world::Kind::Object,
            world::ParentUpdate::Root,
        );
        observation.pose = Some(world::Pose { position: Some(Vec3::new(10.0, 0.0, 0.0)), rotation: Some(math::Quat::IDENTITY) });
        observation.scale = Some(Vec3::ONE);
        observation.shape = world::FieldUpdate::Set(world::Shape::Mesh { asset_id: asset, flags: world::SculptFlags::default() });
        let _applied = world.observe_object(observation).expect("fixture mesh applies");
    }

    let mut mirror = Mirror::default();
    let mut identities = BTreeMap::new();
    let mut cursor = None;
    loop {
        let page = world.object_list_page(cursor, 256).expect("fixture object page reads");
        let next = page.next;
        for item in page.objects {
            let detail = world.object_detail(item.entity).expect("fixture object detail reads");
            let lease = budget.reserve(detail.owned_bytes() + 32).expect("fixture object fits budget");
            mirror.apply(crate::scene::model::Change::Object(crate::scene::Object::new(detail, lease)));
            identities.insert(item.id, item.entity);
        }
        let Some(next) = next else { break };
        cursor = Some(next);
    }
    (budget, mirror, identities)
}

fn mesh_requests(demand: &mut AssetDemand) -> Vec<(AssetId, AssetTicket)> {
    std::iter::from_fn(|| demand.event())
        .filter_map(|change| match change {
            AssetChange::Request(request) => match request.request {
                Request::MeshHeader(asset) => Some((asset, request.ticket)),
                _ => None,
            },
            AssetChange::Release(_)
            | AssetChange::UpdateImportance { .. }
            | AssetChange::UpdatePriority { .. }
            | AssetChange::UpdateSize { .. } => None,
        })
        .collect()
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
            AssetChange::Release(_)
            | AssetChange::UpdateImportance { .. }
            | AssetChange::UpdatePriority { .. }
            | AssetChange::UpdateSize { .. } => None,
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
            AssetChange::UpdateSize { .. } => panic!("replacement texture requests use a new ticket"),
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
            AssetChange::Release(_)
            | AssetChange::UpdateImportance { .. }
            | AssetChange::UpdatePriority { .. }
            | AssetChange::UpdateSize { .. } => None,
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
fn terrain_background_keeps_resolution() {
    let budget = Budget::default();
    let region = Region::new(1);
    let textures = ids(70);
    let mut mirror = Mirror::default();
    mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, textures));
    let close = camera().with_quality(64.0, 1.0).unwrap();
    let mut demand = AssetDemand::new(budget.clone());
    demand.set_camera(close);
    demand.invalidate_region(region);
    process(&mut demand, &mirror);

    let visible_size = demand.terrain_targets[&region].size;
    assert!(visible_size.edge() > 128, "a nearby region requests more than its baseline detail");
    assert_eq!(demand.terrain_targets[&region].priority, Priority::Visible);
    let initial_tickets: Vec<_> = std::iter::from_fn(|| demand.event())
        .filter_map(|change| match change {
            AssetChange::Request(request) => {
                let Request::Texture { id, size } = request.request else { panic!("terrain uses texture requests") };
                assert!(textures.contains(&id.uuid()));
                assert_eq!(size, visible_size);
                assert_eq!(request.priority, Priority::Visible);
                Some(request.ticket)
            }
            AssetChange::Release(_) | AssetChange::UpdateImportance { .. } | AssetChange::UpdatePriority { .. } => None,
            AssetChange::UpdateSize { .. } => panic!("initial terrain demand has no previous size to update"),
        })
        .collect();
    assert_eq!(initial_tickets.len(), 4);

    mirror.regions.insert(
        region,
        RegionData::new(
            RegionSummary { region, info: RegionInfo::default(), terrain_settings: Some(settings(textures)) },
            budget.reserve(512).unwrap(),
        ),
    );
    demand.invalidate_region(region);
    process(&mut demand, &mirror);
    assert_eq!(demand.terrain_targets[&region].size, visible_size, "an unknown origin keeps the last regional texture size");
    assert_eq!(demand.terrain_targets[&region].priority, Priority::Background);
    let background_updates: Vec<_> = std::iter::from_fn(|| demand.event())
        .map(|change| match change {
            AssetChange::UpdatePriority { ticket, priority } => (ticket, priority),
            AssetChange::Request(_) | AssetChange::Release(_) | AssetChange::UpdateImportance { .. } | AssetChange::UpdateSize { .. } => {
                panic!("background terrain keeps its existing texture tickets")
            }
        })
        .collect();
    assert_eq!(background_updates.len(), 4);
    assert!(background_updates.iter().all(|(_, priority)| *priority == Priority::Background));
    assert!(background_updates.iter().all(|(ticket, _)| initial_tickets.contains(ticket)));

    mirror.regions.insert(region, region_data(&budget, region, DVec3::ZERO, textures));
    demand.invalidate_region(region);
    process(&mut demand, &mirror);
    assert_eq!(demand.terrain_targets[&region].size, visible_size);
    assert_eq!(demand.terrain_targets[&region].priority, Priority::Visible);
    let visible_updates: Vec<_> = std::iter::from_fn(|| demand.event())
        .map(|change| match change {
            AssetChange::UpdatePriority { ticket, priority } => (ticket, priority),
            AssetChange::Request(_) | AssetChange::Release(_) | AssetChange::UpdateImportance { .. } | AssetChange::UpdateSize { .. } => {
                panic!("returning terrain keeps its existing texture tickets")
            }
        })
        .collect();
    assert_eq!(visible_updates.len(), 4);
    assert!(visible_updates.iter().all(|(_, priority)| *priority == Priority::Visible));
    assert!(visible_updates.iter().all(|(ticket, _)| initial_tickets.contains(ticket)));

    demand.remove_region(region);
    let released: HashSet<_> = std::iter::from_fn(|| demand.event())
        .map(|change| match change {
            AssetChange::Release(ticket) => ticket,
            AssetChange::Request(_)
            | AssetChange::UpdateImportance { .. }
            | AssetChange::UpdatePriority { .. }
            | AssetChange::UpdateSize { .. } => {
                panic!("region removal releases its retained terrain tickets")
            }
        })
        .collect();
    assert_eq!(released, initial_tickets.into_iter().collect());
    assert!(!demand.terrain_targets.contains_key(&region));
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

#[test]
fn priority_keeps_ticket_until_resize() {
    let region = Region::new(1);
    let id = AssetId::new(Uuid::from_u128(100));
    let size = TextureSize::new(128);
    let user = AssetUser::Terrain(region);
    let mut demand = AssetDemand::new(Budget::default());
    let mut keys = HashSet::new();
    let key = demand.register(user, region, Request::Texture { id, size }, size, Priority::Visible, 600, &mut keys);
    let Some(AssetChange::Request(initial)) = demand.event() else { panic!("texture request is queued") };
    let ticket = initial.ticket;

    for (priority, importance) in [(Priority::Background, 0), (Priority::Visible, 600)] {
        let user_demand =
            demand.entries.get_mut(&key).expect("registered entry exists").users.get_mut(&user).expect("registered user exists");
        user_demand.priority = priority;
        user_demand.importance = importance;

        demand.refresh(key);

        assert_matches!(
            demand.event(),
            Some(AssetChange::UpdatePriority { ticket: updated, priority: actual }) if updated == ticket && actual == priority
        );
        assert_matches!(
            demand.event(),
            Some(AssetChange::UpdateImportance { ticket: updated, importance: actual }) if updated == ticket && actual == importance
        );
        assert!(demand.event().is_none(), "priority and importance updates keep the same request active");
        assert_eq!(demand.tickets.get(&ticket), Some(&key));
    }

    let resized = TextureSize::new(256);
    demand.entries.get_mut(&key).expect("registered entry exists").users.get_mut(&user).expect("registered user exists").size = resized;
    demand.refresh(key);

    assert_matches!(demand.event(), Some(AssetChange::UpdateSize { ticket: updated, size }) if updated == ticket && size == resized);
    assert!(demand.event().is_none());
    let entry = demand.entries.get(&key).expect("registered entry exists");
    assert_eq!(entry.request, Request::Texture { id, size: resized });
    assert_eq!(entry.priority, Priority::Visible);
    assert_eq!(entry.importance, 600);
    assert_eq!(demand.tickets.get(&ticket), Some(&key));
    assert!(demand.event().is_none());
}

#[test]
fn scan_exhaustion_keeps_dirty_work() {
    const OBJECTS: u32 = 128;
    const PROCESSED: u32 = 128;

    let (budget, mirror, identities) = mesh_scene(OBJECTS);
    let first_camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1280, 720]).expect("first camera");
    let next_camera = Camera::new(DVec3::new(5.0, 0.0, 0.0), Vec3::X, 1.0, [1280, 720]).expect("next camera");
    let mut demand = AssetDemand::new(budget);
    demand.set_camera(first_camera);
    let _ = demand.step(&mirror, &identities);
    let initial_requests = mesh_requests(&mut demand);
    let expected_assets: HashSet<_> = (1..=PROCESSED).map(|id| AssetId::new(Uuid::from_u128(10_000 + u128::from(id)))).collect();
    assert_eq!(initial_requests.len(), PROCESSED as usize);
    assert_eq!(initial_requests.iter().map(|(asset, _)| *asset).collect::<HashSet<_>>(), expected_assets);

    demand.set_camera(next_camera);
    for &entity in identities.values() {
        demand.invalidate(mirror.objects.get(&entity).expect("fixture object remains in the scene"));
    }
    let _ = demand.step(&mirror, &identities);
    let updates: Vec<_> = std::iter::from_fn(|| demand.event())
        .map(|change| match change {
            AssetChange::UpdateImportance { ticket, .. } => ticket,
            AssetChange::Request(_) | AssetChange::Release(_) | AssetChange::UpdatePriority { .. } | AssetChange::UpdateSize { .. } => {
                panic!("camera movement updates existing mesh request importance")
            }
        })
        .collect();
    assert_eq!(updates.len(), PROCESSED as usize);
    assert_eq!(updates.into_iter().collect::<HashSet<_>>(), initial_requests.iter().map(|(_, ticket)| *ticket).collect());
}

#[test]
fn dirty_exhaustion_keeps_scan_work() {
    const OBJECTS: u32 = 256;
    const PROCESSED: u32 = 128;
    const SCANNED: u32 = PROCESSED - 1;

    let (budget, mirror, identities) = mesh_scene(OBJECTS);
    let first_camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1280, 720]).expect("first camera");
    let next_camera = Camera::new(DVec3::new(5.0, 0.0, 0.0), Vec3::X, 1.0, [1280, 720]).expect("next camera");
    let dirty_id = Uuid::from_u128(1);
    let dirty_entity = identities[&dirty_id];
    let mut demand = AssetDemand::new(budget);
    demand.set_camera(first_camera);
    let _ = demand.step(&mirror, &identities);
    let initial_requests = mesh_requests(&mut demand);
    assert_eq!(initial_requests.len(), PROCESSED as usize);

    demand.set_camera(next_camera);
    demand.invalidate(mirror.objects.get(&dirty_entity).expect("fixture object remains in the scene"));

    let _ = demand.step(&mirror, &identities);
    let mut requested = Vec::new();
    let mut updates = Vec::new();
    while let Some(change) = demand.event() {
        match change {
            AssetChange::Request(request) => match request.request {
                Request::MeshHeader(asset) => requested.push(asset),
                _ => panic!("fixture mesh work requests mesh headers"),
            },
            AssetChange::UpdateImportance { ticket, .. } => updates.push(ticket),
            AssetChange::Release(_) | AssetChange::UpdatePriority { .. } | AssetChange::UpdateSize { .. } => {
                panic!("camera movement retains mesh requests and priority")
            }
        }
    }
    let expected_assets: HashSet<_> = (129..=129 + SCANNED - 1).map(|id| AssetId::new(Uuid::from_u128(10_000 + u128::from(id)))).collect();
    assert_eq!(requested.len(), SCANNED as usize);
    assert_eq!(requested.into_iter().collect::<HashSet<_>>(), expected_assets);
    let dirty_ticket =
        initial_requests.iter().find_map(|(asset, ticket)| (*asset == AssetId::new(Uuid::from_u128(10_001))).then_some(*ticket));
    assert_eq!(updates, [dirty_ticket.expect("dirty mesh header request")]);
}

#[test]
fn camera_scan_reaches_target() {
    const LOW_USERS: u32 = 1024;
    const TARGET_LOCAL_ID: u32 = LOW_USERS + 1;
    const OBJECTS: usize = TARGET_LOCAL_ID as usize;
    const DIRTY_USERS: u32 = 128;
    const MAX_TURNS: usize = 80;

    let budget = Budget::default();
    let region = Region::new(1);
    let mesh_asset = AssetId::new(Uuid::from_u128(10_000));
    let tombstone_bytes = WorldLimits::tombstone_bytes_for_count(OBJECTS);
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: OBJECTS,
        max_entities_per_region: OBJECTS,
        max_tombstones_per_region: OBJECTS,
        max_tombstones: OBJECTS,
        max_tombstone_bytes_per_region: tombstone_bytes,
        max_tombstone_bytes: tombstone_bytes,
        max_journal_entries: 4096,
        max_journal_changes: 8192,
        max_region_name_bytes: 64,
        max_recovery_targets: 1,
    });
    let _adopted = world.adopt_region(region).expect("fixture region registers");
    let _origin =
        world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("fixture origin applies");

    for local_id in 1..=TARGET_LOCAL_ID {
        let is_target = local_id == TARGET_LOCAL_ID;
        let id = if is_target { Uuid::from_u128(u128::MAX) } else { Uuid::from_u128(u128::from(local_id)) };
        let position = if is_target { Vec3::new(100.0, 0.0, 0.0) } else { Vec3::new(200.0, 0.0, 0.0) };
        let mut observation = world::ObjectObservation::new(
            world::UpdateSource::new(region, local_id, OrderEvidence::Ordered(u64::from(local_id)), 0),
            id,
            std::num::NonZeroU32::new(local_id).expect("fixture local ID is nonzero"),
            world::Kind::Object,
            world::ParentUpdate::Root,
        );
        observation.pose = Some(world::Pose { position: Some(position), rotation: Some(math::Quat::IDENTITY) });
        observation.scale = Some(Vec3::ONE);
        observation.shape = world::FieldUpdate::Set(world::Shape::Mesh { asset_id: mesh_asset, flags: world::SculptFlags::default() });
        let _applied = world.observe_object(observation).expect("fixture mesh applies");
    }

    let mut mirror = Mirror::default();
    let mut identities = BTreeMap::new();
    let mut cursor = None;
    loop {
        let page = world.object_list_page(cursor, 256).expect("fixture object page reads");
        let next = page.next;
        for item in page.objects {
            let detail = world.object_detail(item.entity).expect("fixture object detail reads");
            let lease = budget.reserve(detail.owned_bytes() + 32).expect("fixture object fits budget");
            mirror.apply(crate::scene::model::Change::Object(crate::scene::Object::new(detail, lease)));
            identities.insert(item.id, item.entity);
        }
        let Some(next) = next else { break };
        cursor = Some(next);
    }
    assert_eq!(identities.len(), OBJECTS);

    let first_camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [1280, 720]).expect("first camera");
    let high_camera = Camera::new(DVec3::new(99.5, 0.0, 0.0), Vec3::X, 1.0, [1280, 720]).expect("high camera");
    assert_eq!(high_camera.mesh_lod(DVec3::new(100.0, 0.0, 0.0), Vec3::ONE, None), Some(assets::mesh::MeshLod::High));

    let mut demand = AssetDemand::new(budget.clone());
    demand.set_camera(first_camera);
    for _ in 0..(LOW_USERS as usize / 128) {
        let _ = demand.step(&mirror, &identities);
    }
    let mut shared_header_ticket = None;
    while let Some(change) = demand.event() {
        if let AssetChange::Request(request) = change
            && matches!(request.request, Request::MeshHeader(asset) if asset == mesh_asset)
        {
            assert!(shared_header_ticket.replace(request.ticket).is_none(), "shared users use one mesh header request");
        }
    }
    let shared_header_ticket = shared_header_ticket.expect("low UUID users requested their shared mesh header");

    assert_eq!(invalidate_completed(&mut demand, &mirror, shared_header_ticket, Status::Fetching), LOW_USERS as usize);
    assert_eq!(invalidate_completed(&mut demand, &mirror, shared_header_ticket, Status::Decoding), LOW_USERS as usize);

    demand.set_camera(high_camera);
    let ready_header = ready_header_status(mesh_asset);
    assert_matches!(&ready_header, Status::Ready(assets::Content::MeshHeader(_)));
    assert_eq!(invalidate_completed(&mut demand, &mirror, shared_header_ticket, ready_header), LOW_USERS as usize);

    let mut update_order = u64::from(TARGET_LOCAL_ID);
    let mut requested_high_lod = false;
    for turn in 0..MAX_TURNS {
        for local_id in 1..=DIRTY_USERS {
            update_order += 1;
            let id = Uuid::from_u128(u128::from(local_id));
            let entity = identities[&id];
            let mut patch = world::ObjectPatch::new(
                world::UpdateSource::new(
                    region,
                    u32::try_from(update_order).expect("fixture sequence fits"),
                    OrderEvidence::Ordered(update_order),
                    0,
                ),
                std::num::NonZeroU32::new(local_id).expect("fixture local ID is nonzero"),
            );
            let x = if turn % 2 == 0 { 201.0 } else { 200.0 };
            patch.pose = Some(world::Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(math::Quat::IDENTITY) });
            let _applied = world.patch_object(patch).expect("ordered fixture pose update applies");
            let detail = world.object_detail(entity).expect("updated fixture detail reads");
            let lease = budget.reserve(detail.owned_bytes() + 32).expect("updated fixture object fits budget");
            let object = crate::scene::Object::new(detail, lease);
            mirror.apply(crate::scene::model::Change::Object(object.clone()));
            demand.invalidate(&object);
        }

        let _ = demand.step(&mirror, &identities);
        while let Some(change) = demand.event() {
            if let AssetChange::Request(request) = change
                && let Request::MeshLod(lod) = request.request
                && lod.lod() == assets::mesh::MeshLod::High
            {
                assert_eq!(request.priority, Priority::Visible);
                assert_eq!(lod.asset_id(), mesh_asset);
                requested_high_lod = true;
            }
        }
        if requested_high_lod {
            break;
        }
    }

    assert!(requested_high_lod, "a high UUID target receives its High LOD request under sustained low UUID updates");
}
