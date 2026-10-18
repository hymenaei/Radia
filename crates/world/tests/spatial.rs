use std::{collections::HashSet, num::NonZeroU32};

use base::UUID;
use math::{DVec3, Quat, Vec3};
use world::{
    Applied,
    ApplyOutcome::{Introduced, Removed, Updated},
    Entity, Kind,
    Kind::Object,
    LinearMotion, MAX_QUERY_STATE_MAINTENANCE_WORK, MAX_WORLD_QUERY_RESULTS, MAX_WORLD_QUERY_WORK, MotionPatch, ObjectObservation,
    ObjectPatch,
    OrderEvidence::Ordered,
    ParentUpdate,
    ParentUpdate::{Local, Root},
    Pose, PoseState, QueryCompleteness, QueryIncompleteReason, Region, RegionInfo, SitTarget, UpdateSource, World,
    WorldChange::EntityUpdated,
    WorldChangeClasses, WorldLimits, WorldQuery, WorldQueryBudget, WorldQueryItem,
};

fn world_limits() -> WorldLimits {
    WorldLimits {
        max_region_registrations: 2,
        max_entities: 3,
        max_entities_per_region: 2,
        max_tombstones_per_region: 2,
        max_tombstones: 4,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(4),
        max_journal_entries: 12,
        max_journal_changes: 24,
        max_region_name_bytes: 32,
        max_recovery_targets: 2,
    }
}

fn uuid(value: u128) -> UUID {
    UUID::from_u128(value)
}

fn source(region: Region, order: u64) -> UpdateSource {
    UpdateSource::new(region, order as u32, Ordered(order), 0)
}

fn local_id(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("local ID is nonzero")
}

fn object(region: Region, order: u64, id: u128, local: u32, kind: Kind, parent: ParentUpdate) -> ObjectObservation {
    ObjectObservation::new(source(region, order), uuid(id), local_id(local), kind, parent)
}

fn entity(outcome: Applied) -> Entity {
    let Introduced(entity) = outcome.outcome else { panic!("expected object introduction") };
    entity
}

fn apply_ok<T>(result: Result<T, world::ApplyError>) {
    let _ = result.expect("World update");
}

#[derive(Clone, Copy)]
struct QueryChild {
    id: UUID,
    region: usize,
    local_id: u32,
    parent_local_id: u32,
    position: Vec3,
    pose_valid: bool,
}

fn query_child_local_id(region: usize, slot: usize) -> u32 {
    10 + region as u32 * 16 + slot as u32
}

fn object_with_id(region: Region, order: u64, id: UUID, local: u32, parent: ParentUpdate) -> ObjectObservation {
    let mut observation = object(region, order, 0, local, Object, parent);
    observation.id = id;
    observation
}

struct SpatialModel {
    origins: [Option<DVec3>; 2],
    roots: [[Vec3; 2]; 2],
    root_rotations: [[Quat; 2]; 2],
    children: [Option<QueryChild>; 4],
}

impl SpatialModel {
    fn expected_positions(&self, region_index: usize) -> Option<Vec<(UUID, DVec3)>> {
        let origin = self.origins[region_index]?;
        let mut positions = Vec::new();
        for (root_index, local_position) in self.roots[region_index].iter().enumerate() {
            let position = origin + DVec3::new(local_position.x as f64, local_position.y as f64, local_position.z as f64);
            positions.push((uuid(100 + (region_index * 2 + root_index) as u128), position));
        }
        for child in self.children.iter().flatten().filter(|child| child.region == region_index && child.pose_valid) {
            let parent_index = child.parent_local_id as usize - 1;
            let root = self.roots[region_index][parent_index] + self.root_rotations[region_index][parent_index] * child.position;
            let position = origin + DVec3::new(root.x as f64, root.y as f64, root.z as f64);
            positions.push((child.id, position));
        }
        Some(positions)
    }

    fn expected(&self, region_index: usize, center: DVec3, radius: f64) -> (HashSet<UUID>, usize) {
        let object_count = 2 + self.children.iter().flatten().filter(|child| child.region == region_index).count();
        let Some(positions) = self.expected_positions(region_index) else {
            return (HashSet::new(), object_count);
        };
        let unresolved = object_count - positions.len();
        let located = positions.into_iter().filter_map(|(id, position)| within_radius(position, center, radius).then_some(id)).collect();
        (located, unresolved)
    }
}

fn within_radius(position: DVec3, center: DVec3, radius: f64) -> bool {
    let distance = position - center;
    distance.x.hypot(distance.y).hypot(distance.z) <= radius
}

fn assert_spatial_query_matches(world: &World, regions: [Region; 2], model: &SpatialModel) {
    #[cfg(feature = "invariants")]
    world.check_invariants().expect("World indexes remain consistent");

    for (index, region) in regions.iter().copied().enumerate() {
        let center = model.origins[index].unwrap_or(DVec3::ZERO);
        let query = WorldQuery::live().in_region(region).within(center, 256.0).unwrap();
        let page = world
            .query(query, None, WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: MAX_WORLD_QUERY_RESULTS })
            .expect("spatial query");
        assert_eq!(page.completeness, QueryCompleteness::Exact);
        let actual = page
            .items
            .iter()
            .map(|item| match item {
                WorldQueryItem::Live { object, pose: Some(_), .. } => object.id,
                item => panic!("spatial query returned a non-located live item: {item:?}"),
            })
            .collect::<HashSet<_>>();
        let (expected, unresolved) = model.expected(index, center, 256.0);
        assert_eq!(
            actual,
            expected,
            "region {index}; model positions: {:?}; resolved poses: {:?}",
            model.expected_positions(index),
            world
                .objects()
                .iter()
                .filter(|object| object.address().region() == region)
                .map(|object| (object.id(), object.world_pose()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(page.unresolved.objects, unresolved);
        assert!(page.unresolved.complete);

        if let Some(positions) = model.expected_positions(index) {
            for (id, position) in positions {
                let query = WorldQuery::live().in_region(region).within(position, 0.01).unwrap();
                let page = world
                    .query(query, None, WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: MAX_WORLD_QUERY_RESULTS })
                    .expect("query around modeled object position");
                assert_eq!(page.completeness, QueryCompleteness::Exact);
                assert!(page.items.iter().any(|item| item.id() == id), "spatial query missed {id} at {position:?}");
            }
        }
    }
}

#[test]
fn spatial_work_limit_requires_retry() {
    let region = Region::new(30);
    let mut world = World::new(world_limits());
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    for (order, id, local_id, x) in [(1, 300, 1, 16.0), (2, 301, 2, 16.5)] {
        let mut observation = object(region, order, id, local_id, Object, Root);
        observation.pose = Some(Pose { position: Some(Vec3::new(x, 16.0, 16.0)), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }
    world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap();

    let query = WorldQuery::live().in_region(region).within(DVec3::new(16.0, 16.0, 16.0), 1.0).unwrap();
    let partial = world.query(query, None, WorldQueryBudget { work: 2, results: 8 }).unwrap();
    assert_eq!(partial.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [uuid(300)]);
    assert_eq!(partial.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::WorkLimit });
    assert!(partial.next.is_none(), "spatial query work limits do not provide cursors");

    let complete = world.query(query, None, WorldQueryBudget { work: 8, results: 8 }).unwrap();
    assert_eq!(complete.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [uuid(300), uuid(301)]);
    assert_eq!(complete.completeness, QueryCompleteness::Exact);
    assert!(complete.next.is_none());
}

#[test]
fn sparse_spatial_queries_match_oracle_with_small_work_budget() {
    let region = Region::new(35);
    let mut limits = world_limits();
    limits.max_entities_per_region = 3;
    let mut world = World::new(limits);
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    for (order, id, local_id, x) in [(1, 350, 1, 0.0), (2, 351, 2, 100.0), (3, 352, 3, 1_000.0)] {
        let mut observation = object(region, order, id, local_id, Object, Root);
        observation.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }

    let assert_matches_full_scan = |world: &World, region: Option<Region>, radius: f64| {
        let query =
            region.map_or_else(WorldQuery::live, |region| WorldQuery::live().in_region(region)).within(DVec3::ZERO, radius).unwrap();
        let page = world.query(query, None, WorldQueryBudget { work: 16, results: 8 }).unwrap();
        let expected = world
            .objects()
            .iter()
            .filter(|object| region.is_none_or(|region| object.address().region() == region))
            .filter_map(|object| match object.world_pose().unwrap() {
                PoseState::Resolved(pose) if within_radius(pose.position, DVec3::ZERO, radius) => Some(object.id()),
                PoseState::Resolved(_) | PoseState::Unresolved(_) | PoseState::Provisional { .. } => None,
            })
            .collect::<HashSet<_>>();
        let actual = page.items.iter().map(|item| item.id()).collect::<HashSet<_>>();
        assert_eq!(page.completeness, QueryCompleteness::Exact, "region {region:?}; radius {radius}");
        assert!(page.next.is_none());
        assert!(page.unresolved.complete);
        assert_eq!(page.unresolved.objects, 0);
        assert_eq!(actual, expected, "region {region:?}; radius {radius}");
    };

    for region in [None, Some(region)] {
        for radius in [0.0, 32.0, 256.0, 1_024.0, 1.0e15] {
            assert_matches_full_scan(&world, region, radius);
        }
    }
    let query = WorldQuery::live().within(DVec3::ZERO, 1.0e15).unwrap();
    let partial = world.query(query, None, WorldQueryBudget { work: 2, results: 8 }).unwrap();
    assert_eq!(partial.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::WorkLimit });
    assert!(!partial.unresolved.complete);

    world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap();
    for region in [None, Some(region)] {
        for radius in [0.0, 32.0, 256.0, 1_024.0, 1.0e15] {
            assert_matches_full_scan(&world, region, radius);
        }
    }
}

#[test]
fn unrepresentable_positions_are_not_queryable() {
    let region = Region::new(34);
    let mut world = World::new(world_limits());
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    let mut observation = object(region, 1, 340, 1, Object, Root);
    observation.pose = Some(Pose { position: Some(Vec3::splat(f32::MAX)), rotation: Some(Quat::IDENTITY) });
    let entity = entity(world.observe_object(observation).unwrap());
    world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap();

    let object = world.object(entity).expect("unrepresentable object stays readable");
    let PoseState::Resolved(pose) = object.world_pose().expect("object pose should resolve") else {
        panic!("expected a resolved pose outside the spatial index")
    };
    assert_eq!(pose.position, DVec3::splat(f64::from(f32::MAX)));
    #[cfg(feature = "invariants")]
    assert_eq!(world.check_invariants(), Ok(()));

    let page = world.query(WorldQuery::live().within(DVec3::ZERO, 10.0).unwrap(), None, WorldQueryBudget { work: 16, results: 8 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert!(page.items.is_empty());
    assert_eq!(page.unresolved.objects, 0);
    assert!(matches!(WorldQuery::live().within(DVec3::new(f64::MAX, 0.0, 0.0), 0.0), Err(world::WorldQueryBuildError::OutOfRange)));
}

#[test]
fn motion_and_scale_changes_do_not_dirty_spatial_state() {
    for use_observation in [false, true] {
        let region = Region::new(39);
        let mut world = World::new(world_limits());
        apply_ok(world.adopt_region(region));
        apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
        for local in 1..=2 {
            let parent = if local == 1 { Root } else { Local(local_id(1)) };
            let mut observation = object(region, u64::from(local), u128::from(local), local, Object, parent);
            observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
            apply_ok(world.observe_object(observation));
        }
        assert!(!world.maintain_query_state(4096).unwrap().has_dirty_pressure);
        for (order, expected, work) in [
            (3, WorldChangeClasses::MOTION, 0),
            (4, WorldChangeClasses::SCALE, 0),
            (5, WorldChangeClasses::POSE | WorldChangeClasses::MOTION | WorldChangeClasses::SCALE, 2),
        ] {
            let barrier = world.read_barrier();
            let pose = (order == 5).then_some(Pose { position: Some(Vec3::splat(128.0)), rotation: None });
            let velocity = (order != 4).then_some(Vec3::splat(order as f32));
            let scale = (order != 3).then_some(Vec3::splat(order as f32));
            if use_observation {
                let mut observation = object(region, order, 1, 1, Object, Root);
                observation.pose = pose;
                observation.linear_motion = velocity.map(|velocity| LinearMotion { velocity: Some(velocity), acceleration: None });
                observation.angular_velocity = velocity;
                observation.scale = scale;
                apply_ok(world.observe_object(observation));
            } else {
                let mut patch = ObjectPatch::new(source(region, order), local_id(1));
                patch.pose = pose;
                patch.linear_motion = MotionPatch { velocity, acceleration: None };
                patch.angular_velocity = velocity;
                patch.scale = scale;
                apply_ok(world.patch_object(patch));
            }
            let changes = world.change_page(barrier.changes, 8).unwrap();
            let classes = changes
                .records
                .iter()
                .filter_map(|record| match record.change {
                    EntityUpdated { classes, .. } => Some(classes),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(classes, [expected], "observation={use_observation}, order={order}");
            let step = world.maintain_query_state(4096).unwrap();
            assert_eq!(step.work_used, work, "observation={use_observation}, order={order}");
            assert!(!step.has_dirty_pressure);
            #[cfg(feature = "invariants")]
            assert_eq!(world.check_invariants(), Ok(()));
        }
        let barrier = world.read_barrier();
        apply_ok(world.patch_sit_target(source(region, 6), uuid(1), Some(SitTarget { position: Vec3::X, rotation: Quat::IDENTITY })));
        let changes = world.change_page(barrier.changes, 8).unwrap();
        assert!(matches!(changes.records.as_slice(), [record] if matches!(record.change,
            EntityUpdated { classes, .. } if classes == WorldChangeClasses::RELATIONSHIP)));
        assert_eq!(world.maintain_query_state(1).unwrap().work_used, 0);
    }
}

#[test]
fn isolated_object_repair_does_not_scan_its_region() {
    let region = Region::new(36);
    let mut world_limits = world_limits();
    world_limits.max_entities = 32;
    world_limits.max_entities_per_region = 32;
    let mut world = World::new(world_limits);
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    for local in 1..=32 {
        let mut observation = object(region, u64::from(local), u128::from(local), local, Object, Root);
        observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }
    while world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap().has_dirty_pressure {}
    for order in [33, 34] {
        let mut patch = ObjectPatch::new(source(region, order), local_id(1));
        patch.pose = Some(Pose { position: Some(Vec3::splat(order as f32)), rotation: None });
        apply_ok(world.patch_object(patch));
    }

    let step = world.maintain_query_state(1).unwrap();
    assert_eq!(step.work_used, 1);
    assert_eq!(step.dirty_scopes, 0);
    let query = WorldQuery::live().within(DVec3::splat(34.0), 0.25).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 4, results: 8 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [uuid(1)]);
    #[cfg(feature = "invariants")]
    assert_eq!(world.check_invariants(), Ok(()));
}

#[test]
fn subtree_repair_resumes_after_relationship_changes() {
    let region = Region::new(37);
    let mut limits = world_limits();
    limits.max_entities = 131;
    limits.max_entities_per_region = 131;
    let mut world = World::new(limits);
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    for local in 1..=130 {
        let (id, parent) = match local {
            129 => (10, Local(local_id(1))),
            130 => (20, Local(local_id(1))),
            _ => (1000 + u128::from(local), Root),
        };
        let mut observation = object(region, u64::from(local), id, local, Object, parent);
        observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }
    assert!(!world.maintain_query_state(4096).unwrap().has_dirty_pressure);
    let mut patch = ObjectPatch::new(source(region, 131), local_id(1));
    patch.pose = Some(Pose { position: Some(Vec3::splat(128.0)), rotation: None });
    apply_ok(world.patch_object(patch));
    let query = WorldQuery::live().within(DVec3::splat(128.0), 0.25).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 32, results: 8 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<HashSet<_>>(), HashSet::from([uuid(1001), uuid(10), uuid(20)]));
    let step = world.maintain_query_state(2).unwrap();
    assert_eq!(step.work_used, 2);
    assert!(step.has_dirty_pressure);

    apply_ok(world.observe_object(object(region, 132, 10, 129, Object, Root)));
    let mut child = object(region, 133, 5, 131, Object, Local(local_id(1)));
    child.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
    apply_ok(world.observe_object(child));
    let mut patch = ObjectPatch::new(source(region, 134), local_id(1));
    patch.pose = Some(Pose { position: Some(Vec3::splat(256.0)), rotation: None });
    apply_ok(world.patch_object(patch));

    let mut work = 0;
    let mut done = false;
    for _ in 0..20 {
        let step = world.maintain_query_state(1).unwrap();
        assert!(step.work_used <= 1);
        work += step.work_used;
        #[cfg(feature = "invariants")]
        assert_eq!(world.check_invariants(), Ok(()));
        if !step.has_dirty_pressure {
            done = true;
            break;
        }
    }
    assert!(done, "repair must finish without scanning unrelated objects");
    assert!(work < 12, "only the affected subtree and detached child need repair");
    let query = WorldQuery::live().within(DVec3::splat(256.0), 0.25).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 32, results: 8 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<HashSet<_>>(), HashSet::from([uuid(1001), uuid(5), uuid(20)]));
}

#[test]
fn transferring_a_dirty_root_below_a_deep_parent_clears_its_old_scope() {
    for destination_dirty in [false, true] {
        let first = Region::new(40);
        let second = Region::new(41);
        let mut limits = world_limits();
        limits.max_entities = 19;
        limits.max_entities_per_region = 19;
        let mut world = World::new(limits);
        for region in [first, second] {
            apply_ok(world.adopt_region(region));
            apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
        }
        let root = entity(world.observe_object(object(first, 1, 1000, 1, Object, Root)).unwrap());
        for local in 1..=18 {
            let parent = if local == 1 { Root } else { Local(local_id(local - 1)) };
            let mut observation = object(second, u64::from(local), u128::from(local), local, Object, parent);
            observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
            apply_ok(world.observe_object(observation));
        }
        assert!(!world.maintain_query_state(4096).unwrap().has_dirty_pressure);
        let mut patch = ObjectPatch::new(source(first, 2), local_id(1));
        patch.pose = Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.patch_object(patch));
        if destination_dirty {
            let mut patch = ObjectPatch::new(source(second, 19), local_id(18));
            patch.pose = Some(Pose { position: Some(Vec3::Y), rotation: None });
            apply_ok(world.patch_object(patch));
        }
        apply_ok(world.observe_object(object(second, 20, 1000, 19, Object, Local(local_id(18)))));
        assert_eq!(world.object(root).unwrap().address().region(), second);
        #[cfg(feature = "invariants")]
        assert_eq!(world.check_invariants(), Ok(()));
        let step = world.maintain_query_state(4096).unwrap();
        assert!(!step.has_dirty_pressure);
        assert_eq!(step.work_used, 19, "only the destination region needs repair");
        #[cfg(feature = "invariants")]
        assert_eq!(world.check_invariants(), Ok(()));
    }
}

#[test]
fn subtree_repair_reaches_newly_resolvable_deep_descendants() {
    let region = Region::new(38);
    let mut limits = world_limits();
    limits.max_entities = 34;
    limits.max_entities_per_region = 34;
    let mut world = World::new(limits);
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    for local in 1..=34 {
        let parent = if local == 1 { Root } else { Local(local_id(local - 1)) };
        let mut observation = object(region, u64::from(local), u128::from(local), local, Object, parent);
        observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }
    assert!(!world.maintain_query_state(4096).unwrap().has_dirty_pressure);
    let mut patch = ObjectPatch::new(source(region, 35), local_id(1));
    patch.pose = Some(Pose { position: Some(Vec3::splat(128.0)), rotation: None });
    apply_ok(world.patch_object(patch));
    let query = WorldQuery::live().within(DVec3::splat(128.0), 0.25).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 64, results: 64 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<HashSet<_>>(), (1..=17).map(uuid).collect());
    assert_eq!(page.unresolved.objects, 17);
    assert!(page.unresolved.complete);
    assert!(!world.maintain_query_state(4096).unwrap().has_dirty_pressure);

    let mut root = object(region, 36, 17, 17, Object, Root);
    root.pose = Some(Pose { position: Some(Vec3::splat(256.0)), rotation: Some(Quat::IDENTITY) });
    apply_ok(world.observe_object(root));
    let query = WorldQuery::live().within(DVec3::splat(256.0), 0.25).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 64, results: 64 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<HashSet<_>>(), (17..=33).map(uuid).collect());
    assert_eq!(page.unresolved.objects, 1);
    assert!(page.unresolved.complete);
    let mut work = 0;
    for _ in 0..20 {
        let step = world.maintain_query_state(1).unwrap();
        work += step.work_used;
        if !step.has_dirty_pressure {
            break;
        }
    }
    assert_eq!(work, 17, "only descendants within the pose depth limit need repair");
    let query = WorldQuery::live().within(DVec3::splat(256.0), 0.25).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 128, results: 64 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<HashSet<_>>(), (17..=33).map(uuid).collect());
    assert_eq!(page.unresolved.objects, 1);
    #[cfg(feature = "invariants")]
    assert_eq!(world.check_invariants(), Ok(()));
}

#[test]
fn repeated_root_updates_keep_scope() {
    let region = Region::new(33);
    let mut world = World::new(world_limits());
    apply_ok(world.adopt_region(region));
    apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    let root = entity(world.observe_object(object(region, 1, 330, 1, Object, Root)).unwrap());
    let mut child = object(region, 2, 331, 2, Object, Local(local_id(1)));
    child.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
    apply_ok(world.observe_object(child));
    world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap();

    for (order, x) in [(3, 128.0), (4, 256.0)] {
        let mut patch = ObjectPatch::new(source(region, order), local_id(1));
        patch.pose = Some(Pose { position: Some(Vec3::new(x, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.patch_object(patch));
    }

    let query = WorldQuery::live().within(DVec3::new(256.0, 0.0, 0.0), 1.0).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: 8 }).unwrap();

    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [uuid(330), uuid(331)]);
    assert_eq!(world.object(root).unwrap().position(), Some(Vec3::new(256.0, 0.0, 0.0)));
}

#[test]
fn many_regions_repair_incrementally() {
    const REGION_COUNT: usize = 40;
    let mut world = World::new(WorldLimits { max_region_registrations: REGION_COUNT, max_entities: REGION_COUNT, ..world_limits() });
    let mut ids = Vec::with_capacity(REGION_COUNT);

    for index in 0..REGION_COUNT {
        let region = Region::new(100 + index as u64);
        apply_ok(world.adopt_region(region));
        apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
        let mut observation = object(region, 1, 10_000 + index as u128, 1, Object, Root);
        observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
        ids.push(uuid(10_000 + index as u128));
    }

    assert_eq!(world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap().dirty_scopes, 0);
    for index in 0..REGION_COUNT {
        let region = Region::new(100 + index as u64);
        let mut patch = ObjectPatch::new(source(region, 2), local_id(1));
        patch.pose = Some(Pose { position: Some(Vec3::new(index as f32 + 1.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        let applied = world.patch_object(patch).unwrap();
        assert!(matches!(applied.outcome, Updated(_)), "each region root should move");
    }

    let maintenance = world.maintain_query_state(1).unwrap();
    assert!(maintenance.dirty_scopes > 0, "the bounded repair should leave query state dirty");

    let query = WorldQuery::live().within(DVec3::new(21.0, 0.0, 0.0), 20.0).unwrap();
    let page = world.query(query, None, WorldQueryBudget { work: 1_000, results: REGION_COUNT }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.iter().map(|item| (*item).id()).collect::<HashSet<_>>(), ids.iter().copied().collect());

    let mut maintenance = world.maintain_query_state(1).unwrap();
    for _ in 0..(REGION_COUNT * 3) {
        assert!(maintenance.work_used <= 1);
        if maintenance.dirty_scopes == 0 {
            break;
        }
        maintenance = world.maintain_query_state(1).unwrap();
    }
    assert_eq!(maintenance.dirty_scopes, 0);

    let repaired = world.query(query, None, WorldQueryBudget { work: 1_000, results: REGION_COUNT }).unwrap();
    assert_eq!(repaired.completeness, QueryCompleteness::Exact);
    assert_eq!(repaired.items.iter().map(|item| item.id()).collect::<HashSet<_>>(), ids.iter().copied().collect());

    for index in 0..REGION_COUNT {
        let region = Region::new(100 + index as u64);
        let mut patch = ObjectPatch::new(source(region, 3), local_id(1));
        patch.pose = Some(Pose { position: Some(Vec3::new(index as f32 + 2.0, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        assert!(matches!(world.patch_object(patch).unwrap().outcome, Updated(_)));
        assert_eq!(world.remove_object(source(region, 4), local_id(1)).unwrap().outcome, Removed(uuid(10_000 + index as u128)));
    }
    let maintenance = world.maintain_query_state(1).unwrap();
    assert_eq!(maintenance.work_used, 0);
    assert_eq!(maintenance.dirty_scopes, 0);
    assert_eq!(world.object_count(), 0);
}

#[test]
fn empty_region_repair_is_bounded() {
    const REGION_COUNT: usize = 24;
    let limits = WorldLimits { max_region_registrations: REGION_COUNT, ..world_limits() };
    let mut world = World::new(limits);
    for index in 0..REGION_COUNT {
        let region = Region::new(300 + index as u64);
        apply_ok(world.adopt_region(region));
        apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
    }

    let maintenance = world.maintain_query_state(1).unwrap();

    assert_eq!(maintenance.work_used, 0);
    assert_eq!(maintenance.dirty_scopes, REGION_COUNT - 1);
}

#[test]
fn hot_region_repair_is_fair() {
    const OTHER_REGIONS: usize = 8;
    const HOT_ROOTS: usize = 49;
    const HOT_REGION: Region = Region::new(200);

    let mut limits = world_limits();
    limits.max_region_registrations = OTHER_REGIONS + 1;
    limits.max_entities = HOT_ROOTS + OTHER_REGIONS;
    limits.max_entities_per_region = HOT_ROOTS;
    let mut world = World::new(limits);
    apply_ok(world.adopt_region(HOT_REGION));
    apply_ok(world.patch_region(HOT_REGION, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));

    for local in 1..=HOT_ROOTS {
        let mut observation = object(HOT_REGION, local as u64, 20_000 + local as u128, local as u32, Object, Root);
        observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }
    for index in 0..OTHER_REGIONS {
        let region = Region::new(201 + index as u64);
        apply_ok(world.adopt_region(region));
        apply_ok(world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }));
        let mut observation = object(region, 1, 21_000 + index as u128, 1, Object, Root);
        observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.observe_object(observation));
    }
    assert_eq!(world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).unwrap().dirty_scopes, 0);

    for index in 0..OTHER_REGIONS {
        let region = Region::new(201 + index as u64);
        let mut patch = ObjectPatch::new(source(region, 2), local_id(1));
        let position = Vec3::new(48.0 + index as f32 * 32.0, 48.0, 48.0);
        patch.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.patch_object(patch));
    }
    for local in 1..=HOT_ROOTS {
        let mut patch = ObjectPatch::new(source(HOT_REGION, (HOT_ROOTS + local) as u64), local_id(local as u32));
        patch.pose = Some(Pose { position: Some(Vec3::new(local as f32, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.patch_object(patch));
    }

    let dirty_query = WorldQuery::live().in_region(HOT_REGION).within(DVec3::ZERO, 100.0).unwrap();
    let page = world.query(dirty_query, None, WorldQueryBudget { work: 1_000, results: 64 }).unwrap();
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.len(), HOT_ROOTS);
    let actual = page.items.iter().map(|item| item.id()).collect::<HashSet<_>>();
    let expected = (0..HOT_ROOTS).map(|index| uuid(20_001 + index as u128)).collect::<HashSet<_>>();
    assert_eq!(actual, expected);

    let index_probe_budget = WorldQueryBudget { work: 3, results: 2 };
    let dirty_region = Region::new(201);
    let dirty_region_query = WorldQuery::live().in_region(dirty_region).within(DVec3::new(48.0, 48.0, 48.0), 0.25).unwrap();
    let dirty_region_page = world.query(dirty_region_query, None, index_probe_budget).unwrap();
    assert_eq!(dirty_region_page.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::WorkLimit });
    assert_eq!(dirty_region_page.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [uuid(21_000)]);

    let mut maintenance = world.maintain_query_state(1).unwrap();
    for order in (HOT_ROOTS * 2 + 1)..(HOT_ROOTS * 2 + 41) {
        let mut patch = ObjectPatch::new(source(HOT_REGION, order as u64), local_id(1));
        patch.pose = Some(Pose { position: Some(Vec3::new(1_000.0 + order as f32, 0.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        apply_ok(world.patch_object(patch));
        maintenance = world.maintain_query_state(1).unwrap();
        assert!(maintenance.work_used <= 1);
    }
    assert_eq!(maintenance.dirty_scopes, 1, "other-region root scopes should finish while the hot region stays dirty");

    for index in 0..OTHER_REGIONS {
        let region = Region::new(201 + index as u64);
        let position = 48.0 + index as f64 * 32.0;
        let query = WorldQuery::live().in_region(region).within(DVec3::new(position, 48.0, 48.0), 0.25).unwrap();
        let page = world.query(query, None, index_probe_budget).unwrap();
        assert_eq!(page.completeness, QueryCompleteness::Exact, "repaired index should answer the moved object in {region:?}");
        assert_eq!(page.items.iter().map(|item| item.id()).collect::<Vec<_>>(), [uuid(21_000 + index as u128)]);
    }

    let mut repair_work = 0;
    for _ in 0..400 {
        if maintenance.dirty_scopes == 0 {
            break;
        }
        maintenance = world.maintain_query_state(1).unwrap();
        repair_work += maintenance.work_used;
        assert!(maintenance.work_used <= 1);
    }
    assert_eq!(maintenance.dirty_scopes, 0);
    assert!(repair_work >= HOT_ROOTS, "the remaining scope should be the large hot region");

    let repaired = world.query(dirty_query, None, WorldQueryBudget { work: 1_000, results: HOT_ROOTS }).unwrap();
    assert_eq!(repaired.completeness, QueryCompleteness::Exact);
    let repaired_ids = repaired.items.iter().map(|item| item.id()).collect::<HashSet<_>>();
    let expected_ids = (2..=HOT_ROOTS).map(|local| uuid(20_000 + local as u128)).collect::<HashSet<_>>();
    assert_eq!(repaired_ids, expected_ids);
}

#[test]
fn recent_unresolved_obeys_budget() {
    let region = Region::new(35);
    let mut world = World::new(world_limits());
    apply_ok(world.adopt_region(region));
    let mut order = 1;
    for local in 1..=2 {
        apply_ok(world.observe_object(object(region, order, 350 + local as u128, local, Object, Root)));
        order += 1;
        apply_ok(world.remove_object(source(region, order), local_id(local)));
        order += 1;
    }
    let query = WorldQuery::recent().within(DVec3::ZERO, 1.0).unwrap();

    let complete = world.query(query, None, WorldQueryBudget { work: 8, results: 8 }).unwrap();
    assert_eq!(complete.completeness, QueryCompleteness::Exact);
    assert_eq!(complete.unresolved.objects, 2);
    assert!(complete.unresolved.complete);

    let partial = world.query(query, None, WorldQueryBudget { work: 1, results: 8 }).unwrap();
    assert_eq!(partial.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::WorkLimit });
    assert_eq!(partial.unresolved.objects, 1);
    assert!(!partial.unresolved.complete);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 48, max_shrink_iters: 128, .. proptest::test_runner::Config::default() })]

    #[test]
    fn spatial_updates_match_oracle(
        operations in proptest::collection::vec((0_u8..8, 0_usize..4, -60_i16..61, -60_i16..61, -60_i16..61), 1..48)
    ) {
        let regions = [Region::new(40), Region::new(41)];
        let mut limits = world_limits();
        limits.max_region_registrations = 2;
        limits.max_entities = 16;
        limits.max_entities_per_region = 12;
        limits.max_tombstones_per_region = 32;
        limits.max_tombstones = 64;
        limits.max_tombstone_bytes_per_region = WorldLimits::tombstone_bytes_for_count(32);
        limits.max_tombstone_bytes = WorldLimits::tombstone_bytes_for_count(64);
        limits.max_journal_entries = 128;
        limits.max_journal_changes = 8192;
        let mut world = World::new(limits);
        let mut order = 0_u64;
        let mut model = SpatialModel {
            origins: [None, None],
            roots: [[Vec3::ZERO; 2]; 2],
            root_rotations: [[Quat::IDENTITY; 2]; 2],
            children: [None; 4],
        };
        for region in regions {
            apply_ok(world.adopt_region(region));
        }
        for (region_index, region) in regions.iter().copied().enumerate() {
            for root_index in 0..2 {
                let local = root_index as u32 + 1;
                order += 1;
                let mut observation = object(region, order, 100 + (region_index * 2 + root_index) as u128, local, Object, Root);
                observation.pose = Some(Pose { position: Some(Vec3::ZERO), rotation: Some(Quat::IDENTITY) });
                apply_ok(world.observe_object(observation));
            }
        }

        let mut next_id = 200_u128;
        for (slot, child) in model.children.iter_mut().enumerate() {
            let region = slot % regions.len();
            let local = query_child_local_id(region, slot);
            let parent_local_id = (slot % 2 + 1) as u32;
            let position = Vec3::new(slot as f32 * 2.0, 1.0, 0.0);
            let id_value = next_id;
            let id = uuid(id_value);
            next_id += 1;
            order += 1;
            let mut observation = object(
                regions[region],
                order,
                id_value,
                local,
                Object,
                Local(local_id(parent_local_id)),
            );
            observation.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
            apply_ok(world.observe_object(observation));
            *child = Some(QueryChild { id, region, local_id: local, parent_local_id, position, pose_valid: true });
        }

        assert_spatial_query_matches(&world, regions, &model);
        let required = [
            (0_u8, 0_usize, 20_i16, -10_i16, 5_i16),
            (1, 0, 15, 30, 0),
            (2, 0, 1, 0, 0),
            (3, 0, 0, 0, 0),
            (4, 0, 0, 0, 0),
            (5, 1, 0, 0, 0),
            (6, 2, 0, 0, 0),
            (7, 0, 5, 3, 1),
        ];
        for (operation, target, x, y, z) in required.into_iter().chain(operations) {
            let slot = target;
            let position = Vec3::new(x as f32, y as f32, z as f32);
            match operation {
                0 => if let Some(child) = &mut model.children[slot] {
                    order += 1;
                    let mut patch = ObjectPatch::new(source(regions[child.region], order), local_id(child.local_id));
                    patch.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
                    apply_ok(world.patch_object(patch));
                    child.position = position;
                    child.pose_valid = true;
                },
                1 => {
                    let root_region = slot % regions.len();
                    let root_index = (slot / regions.len()) % 2;
                    let root_local = root_index as u32 + 1;
                    let rotation = Quat::from_rotation_z(y as f32 * std::f32::consts::PI / 180.0);
                    order += 1;
                    let mut patch = ObjectPatch::new(source(regions[root_region], order), local_id(root_local));
                    patch.pose = Some(Pose { position: Some(position), rotation: Some(rotation) });
                    apply_ok(world.patch_object(patch));
                    model.roots[root_region][root_index] = position;
                    model.root_rotations[root_region][root_index] = rotation;
                },
                2 => if let Some(child) = &mut model.children[slot] {
                    let parent_local_id = (x.unsigned_abs() as usize % 2 + 1) as u32;
                    if child.parent_local_id != parent_local_id {
                        child.pose_valid = false;
                    }
                    order += 1;
                    let mut patch = ObjectPatch::new(source(regions[child.region], order), local_id(child.local_id));
                    patch.parent = Some(Local(local_id(parent_local_id)));
                    apply_ok(world.patch_object(patch));
                    child.parent_local_id = parent_local_id;
                },
                3 => {
                    let region_index = slot % regions.len();
                    if model.origins[region_index].is_none() {
                        let origin = DVec3::new(region_index as f64 * 1024.0, 0.0, 0.0);
                        apply_ok(world.patch_region(regions[region_index], RegionInfo { origin: Some(origin), ..RegionInfo::default() }));
                        model.origins[region_index] = Some(origin);
                    }
                },
                4 => if let Some(child) = model.children[slot].take() {
                    order += 1;
                    apply_ok(world.remove_object(source(regions[child.region], order), local_id(child.local_id)));
                },
                5 => if let Some(child) = &mut model.children[slot] {
                    order += 1;
                    apply_ok(world.remove_object(source(regions[child.region], order), local_id(child.local_id)));
                    next_id += 1;
                    let id = uuid(next_id);
                    order += 1;
                    let mut observation = object_with_id(
                        regions[child.region],
                        order,
                        id,
                        child.local_id,
                        Local(local_id(child.parent_local_id)),
                    );
                    observation.pose = Some(Pose { position: Some(child.position), rotation: Some(Quat::IDENTITY) });
                    apply_ok(world.observe_object(observation));
                    child.id = id;
                    child.pose_valid = true;
                },
                6 => if let Some(child) = &mut model.children[slot] {
                    let destination = 1 - child.region;
                    let destination_local_id = query_child_local_id(destination, slot);
                    let parent_local_id = (slot % 2 + 1) as u32;
                    order += 1;
                    let mut observation = object_with_id(
                        regions[destination],
                        order,
                        child.id,
                        destination_local_id,
                        Local(local_id(parent_local_id)),
                    );
                    observation.pose = Some(Pose { position: Some(child.position), rotation: Some(Quat::IDENTITY) });
                    apply_ok(world.observe_object(observation));
                    child.region = destination;
                    child.local_id = destination_local_id;
                    child.parent_local_id = parent_local_id;
                    child.pose_valid = true;
                },
                7 => if model.children[slot].is_none() {
                    let region_index = slot % regions.len();
                    let local = query_child_local_id(region_index, slot);
                    let parent_local_id = (slot % 2 + 1) as u32;
                    next_id += 1;
                    let id = uuid(next_id);
                    order += 1;
                    let mut observation = object_with_id(
                        regions[region_index],
                        order,
                        id,
                        local,
                        Local(local_id(parent_local_id)),
                    );
                    observation.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
                    apply_ok(world.observe_object(observation));
                    model.children[slot] = Some(QueryChild { id, region: region_index, local_id: local, parent_local_id, position, pose_valid: true });
                },
                _ => unreachable!(),
            }

            assert_spatial_query_matches(&world, regions, &model);
            let maintenance_work = (target % 16) + 1;
            world.maintain_query_state(maintenance_work).unwrap();
            assert_spatial_query_matches(&world, regions, &model);
        }
    }
}
