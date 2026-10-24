use std::{hint::black_box, num::NonZeroU32};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use world::{
    ApplyOutcome::{Introduced, Updated},
    ObjectPatch, Parent,
    ParentUpdate::{Local, Root},
    World,
};

use super::support::{INITIAL_ORDER, LOAD_SIZES, PER_REGION, Row, empty_scene, observation, size_name, source};

struct LinkedScene {
    world: World,
    children: Vec<Row>,
    next_order: u64,
}

pub(super) fn bench(c: &mut Criterion) {
    bench_parent_churn(c);
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

fn linked_scene(count: usize) -> LinkedScene {
    let mut scene = empty_scene(count, false);

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
    for root in &roots {
        let root_entity = scene.world.entity(root.id).expect("linkset root");
        let expected_children = scene.rows.iter().filter(|row| row.region == root.region).count() - 1;
        assert_eq!(scene.world.children(root_entity).count(), expected_children);
    }

    LinkedScene { world: scene.world, children, next_order: INITIAL_ORDER + 1_000_000 }
}
