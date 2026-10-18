use std::{hint::black_box, num::NonZeroU32};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use math::{DVec3, Vec3};
use world::{
    ApplyOutcome::{Introduced, Updated},
    MAX_QUERY_STATE_MAINTENANCE_WORK, MAX_WORLD_QUERY_RESULTS, MAX_WORLD_QUERY_WORK, ObjectPatch, Parent,
    ParentUpdate::{Local, Root},
    Pose, QueryCompleteness, QueryIncompleteReason, RegionInfo, World, WorldQuery, WorldQueryBudget,
};

use super::support::{INITIAL_ORDER, LOAD_SIZES, PER_REGION, Row, empty_scene, observation, rows, size_name, source};

const LINKED_READ_WORK: usize = 4_096;
const LINKED_READ_RESULTS: usize = 256;

struct LinkedScene {
    world: World,
    children: Vec<Row>,
    next_order: u64,
}

pub(super) fn bench(c: &mut Criterion) {
    bench_bounded_query(c);
    bench_parent_churn(c);
    bench_spatial_repair(c);
}

fn bench_bounded_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/load_query");
    group.throughput(Throughput::Elements(MAX_WORLD_QUERY_RESULTS as u64));

    for &count in LOAD_SIZES {
        let scene = linked_scene(count);
        let query = WorldQuery::live();
        let budget = WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: MAX_WORLD_QUERY_RESULTS };
        let page = scene.world.query(query, None, budget).expect("bounded World query");
        assert_eq!(page.items.len(), MAX_WORLD_QUERY_RESULTS);
        assert_eq!(page.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::ResultLimit });

        group.bench_with_input(BenchmarkId::new("first_page", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(scene.world.query(query, None, budget).expect("bounded World query")));
        });
    }

    group.finish();
}

fn bench_parent_churn(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/linkset_churn");

    for &count in LOAD_SIZES {
        let child_count = count - count.div_ceil(PER_REGION);
        group.throughput(Throughput::Elements((child_count * 2) as u64));
        let mut verification = linked_scene(count);
        let child = verification.children[0];
        let child_entity = verification.world.entity(child.id).expect("linkset child");
        let root_id = verification
            .world
            .objects_in_region(child.region)
            .find(|object| object.address().local_id().get() == 1)
            .expect("linkset root")
            .id();
        let root_entity = verification.world.entity(root_id).expect("root entity");
        let mut patch = ObjectPatch::new(source(child.region, verification.next_order), child.local_id);
        verification.next_order += 1;
        patch.parent = Some(Root);
        assert_eq!(verification.world.patch_object(patch).expect("detach linkset child").outcome, Updated(child_entity));
        assert_eq!(verification.world.object(child_entity).unwrap().parent(), Parent::Root);
        assert!(!verification.world.children(root_entity).any(|object| object.id() == child.id));
        let mut patch = ObjectPatch::new(source(child.region, verification.next_order), child.local_id);
        verification.next_order += 1;
        patch.parent = Some(Local(NonZeroU32::new(1).expect("root local ID")));
        assert_eq!(verification.world.patch_object(patch).expect("reattach linkset child").outcome, Updated(child_entity));
        assert_eq!(verification.world.object(child_entity).unwrap().parent(), Parent::Resolved(root_entity));
        assert!(verification.world.children(root_entity).any(|object| object.id() == child.id));

        group.bench_with_input(BenchmarkId::new("reparent_children", size_name(count)), &count, |b, _| {
            b.iter_batched_ref(
                || linked_scene(count),
                |scene| {
                    for parent in [Root, Local(NonZeroU32::new(1).expect("root local ID"))] {
                        for row in &scene.children {
                            let mut patch = ObjectPatch::new(source(row.region, scene.next_order), row.local_id);
                            scene.next_order += 1;
                            patch.parent = Some(parent);
                            let _ = black_box(scene.world.patch_object(patch).expect("reparent linkset child"));
                        }
                    }
                    black_box(scene.world.object_count());
                },
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

fn bench_spatial_repair(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/spatial_repair");

    for &count in LOAD_SIZES {
        let (rows, regions) = rows(count, 0);
        let roots = rows.into_iter().filter(|row| row.local_id.get() == 1).collect::<Vec<_>>();

        let mut verification = dirty_linked_world(count, &roots);
        repair(&mut verification);
        for region in &regions {
            let query = WorldQuery::live().in_region(*region).within(region_origin(*region), 0.25).expect("valid bounded spatial query");
            let exact = verification
                .query(query, None, WorldQueryBudget { work: MAX_WORLD_QUERY_WORK, results: MAX_WORLD_QUERY_RESULTS })
                .expect("query repaired spatial index");
            assert!(exact.items.is_empty());
            assert_eq!(exact.completeness, QueryCompleteness::Exact);
            assert!(exact.unresolved.complete);
        }
        drop(verification);

        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("dirty_linksets", size_name(count)), &count, |b, _| {
            b.iter_batched_ref(
                || dirty_linked_world(count, &roots),
                |world| {
                    let mut work = 0;
                    let mut calls = 0;
                    loop {
                        let step = world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("bounded repair");
                        work += step.work_used;
                        calls += 1;
                        if step.dirty_scopes == 0 {
                            break;
                        }
                    }
                    black_box((work, calls, world.object_count()));
                },
                BatchSize::LargeInput,
            );
        });
    }

    group.finish();
}

fn linked_scene(count: usize) -> LinkedScene {
    let mut scene = empty_scene(count, false);
    for region in &scene.regions {
        let _ = scene
            .world
            .patch_region(*region, RegionInfo { origin: Some(region_origin(*region)), ..RegionInfo::default() })
            .expect("set region origin");
    }

    let mut roots = Vec::with_capacity(scene.regions.len());
    let mut children = Vec::with_capacity(count - scene.regions.len());
    for row in &scene.rows {
        let mut object = observation(*row);
        if row.local_id.get() == 1 {
            roots.push(*row);
        } else {
            object.parent = Local(NonZeroU32::new(1).expect("root local ID"));
            children.push(*row);
        }
        assert!(matches!(scene.world.observe_object(object).expect("insert linked object").outcome, Introduced(_)));
    }
    repair(&mut scene.world);

    for root in &roots {
        let root_entity = scene.world.entity(root.id).expect("linkset root");
        let expected_children = scene.rows.iter().filter(|row| row.region == root.region).count() - 1;
        assert_eq!(scene.world.children(root_entity).count(), expected_children);
    }

    LinkedScene { world: scene.world, children, next_order: INITIAL_ORDER + 1_000_000 }
}

fn move_roots(scene: &mut LinkedScene, roots: &[Row]) {
    for root in roots {
        let mut patch = ObjectPatch::new(source(root.region, scene.next_order), root.local_id);
        scene.next_order += 1;
        patch.pose = Some(Pose { position: Some(Vec3::ONE), rotation: Some(math::Quat::IDENTITY) });
        let _ = black_box(scene.world.patch_object(patch).expect("move linkset root"));
    }
}

fn dirty_linked_world(count: usize, roots: &[Row]) -> World {
    let mut scene = linked_scene(count);
    move_roots(&mut scene, roots);

    let query =
        WorldQuery::live().in_region(roots[0].region).within(region_origin(roots[0].region), 0.25).expect("valid bounded spatial query");
    let incomplete = scene
        .world
        .query(query, None, WorldQueryBudget { work: LINKED_READ_WORK, results: LINKED_READ_RESULTS })
        .expect("query dirty spatial index");
    assert!(incomplete.items.is_empty());
    assert_eq!(incomplete.completeness, QueryCompleteness::Incomplete { reason: QueryIncompleteReason::WorkLimit });
    assert!(!incomplete.unresolved.complete);
    scene.world
}

fn repair(world: &mut World) {
    loop {
        let step = world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("maintain query state");
        if step.dirty_scopes == 0 {
            return;
        }
    }
}

fn region_origin(region: world::Region) -> DVec3 {
    DVec3::new(region.get() as f64 * 1_024.0, 0.0, 0.0)
}
