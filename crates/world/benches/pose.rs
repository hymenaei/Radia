use std::{hint::black_box, num::NonZeroU32};

use base::UUID;
use criterion::{BenchmarkId, Criterion, Throughput};
use math::{DVec3, Quat, Vec3};
use world::{
    ApplyOutcome::{Introduced, RegionAdopted, RegionPatched},
    Entity, FieldGroup, FieldRejection,
    FieldRejectionReason::ParentCycle,
    Kind::Object,
    ObjectObservation, ObjectPatch,
    OrderEvidence::Ordered,
    Parent,
    ParentUpdate::{Local, Root},
    Pose, PoseError, PoseState, Region, RegionInfo, UpdateSource, World, WorldLimits,
};

const DEPTHS: &[(&str, usize)] = &[("root", 0), ("typical", 8), ("limit", 16), ("over_limit", 17)];

struct Scenario {
    name: &'static str,
    edge_depth: usize,
    world: World,
    target: Entity,
}

fn world_with_chain(edge_depth: usize) -> (World, Entity) {
    let entity_count = edge_depth + 1;
    let mut world = World::new(WorldLimits {
        max_region_registrations: 1,
        max_entities: entity_count,
        max_entities_per_region: entity_count,
        max_tombstones_per_region: 1,
        max_tombstones: 1,
        max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
        max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
        max_journal_entries: entity_count,
        max_journal_changes: entity_count.saturating_mul(4),
        max_region_name_bytes: 0,
        max_recovery_targets: 0,
    });
    let region = Region::new(1);
    assert_eq!(world.adopt_region(region).expect("region adoption").outcome, RegionAdopted);
    assert_eq!(
        world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region origin").outcome,
        RegionPatched
    );

    let mut target = None;
    for index in 0..entity_count {
        let index = u32::try_from(index).expect("benchmark chain index fits u32");
        let local_id = NonZeroU32::new(index + 1).expect("local ID is nonzero");
        let parent = if index == 0 { Root } else { Local(NonZeroU32::new(index).expect("parent local ID is nonzero")) };
        let source = UpdateSource::new(region, index + 1, Ordered(u64::from(index) + 1), 0);
        let observation = ObjectObservation {
            source,
            id: UUID::from_u128(u128::from(index) + 1),
            local_id,
            kind: Object,
            parent,
            pose: Some(Pose { position: Some(Vec3::X), rotation: Some(Quat::IDENTITY) }),
            linear_motion: None,
            angular_velocity: None,
            scale: None,
            attachment_point: None,
            shape: None,
            appearance: None,
            surface_appearance: None,
            render_materials: None,
        };
        let applied = world.observe_object(observation).expect("object observation");
        let Introduced(entity) = applied.outcome else { panic!("expected object introduction") };
        target = Some(entity);
    }

    (world, target.expect("chain contains a root"))
}

fn cycle_patch(region: Region, order: u64, root_local_id: NonZeroU32, tail_local_id: NonZeroU32) -> ObjectPatch {
    let mut patch = ObjectPatch::new(
        UpdateSource::new(region, u32::try_from(order).expect("benchmark order fits u32"), Ordered(order), 0),
        root_local_id,
    );
    patch.parent = Some(Local(tail_local_id));
    patch
}

fn assert_chain_depth(world: &World, target: Entity, edge_depth: usize) {
    let mut current = target;
    for _ in 0..edge_depth {
        let Parent::Resolved(parent) = world.object(current).expect("chain object").parent() else {
            panic!("parent chain ended before its measured depth");
        };
        current = parent;
    }
    assert!(matches!(world.object(current).map(|object| object.parent()), Some(Parent::Root)));
}

fn assert_pose_contract(scenario: &Scenario) {
    let object = scenario.world.object(scenario.target).expect("target object");
    match (object.world_pose(), object.coordinate_frame()) {
        (Ok(PoseState::Resolved(_)), Ok(_)) if scenario.edge_depth <= 16 => {}
        (Err(PoseError::TraversalLimitExceeded { max_edges: 16 }), Err(PoseError::TraversalLimitExceeded { max_edges: 16 }))
            if scenario.edge_depth > 16 => {}
        _ => panic!("unexpected pose result at {} edges", scenario.edge_depth),
    }
}

pub(super) fn bench(c: &mut Criterion) {
    let mut scenarios = DEPTHS
        .iter()
        .copied()
        .map(|(name, edge_depth)| {
            let (world, target) = world_with_chain(edge_depth);
            assert_chain_depth(&world, target, edge_depth);
            let scenario = Scenario { name, edge_depth, world, target };
            assert_pose_contract(&scenario);
            scenario
        })
        .collect::<Vec<_>>();

    let mut group = c.benchmark_group("world/pose");
    for scenario in &scenarios {
        let object = scenario.world.object(scenario.target).expect("target object");
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("resolve", scenario.name), &scenario.edge_depth, |b, _| {
            b.iter(|| black_box(object.world_pose()));
        });
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("coordinate_frame", scenario.name), &scenario.edge_depth, |b, _| {
            b.iter(|| black_box(object.coordinate_frame()));
        });
    }
    for scenario in &mut scenarios {
        let region = Region::new(1);
        let root_local_id = NonZeroU32::new(1).expect("root local ID is nonzero");
        let tail_local_id =
            NonZeroU32::new(u32::try_from(scenario.edge_depth + 1).expect("tail local ID fits u32")).expect("tail local ID is nonzero");
        let mut order = u64::try_from(scenario.edge_depth + 2).expect("cycle order fits u64");
        let probe = scenario
            .world
            .patch_object(cycle_patch(region, order, root_local_id, tail_local_id))
            .expect("cycle proposal is a field rejection");
        assert_eq!(probe.rejections.len(), 1);
        assert_eq!(probe.rejections.iter().next().copied(), Some(FieldRejection { group: FieldGroup::Parent, reason: ParentCycle }));
        order += 1;

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("cycle_proposal", scenario.name), &scenario.edge_depth, |b, _| {
            b.iter(|| {
                let current_order = order;
                order += 1;
                let _ = black_box(
                    scenario
                        .world
                        .patch_object(cycle_patch(region, current_order, root_local_id, tail_local_id))
                        .expect("cycle proposal is rejected"),
                );
            });
        });
    }
    group.finish();
}
