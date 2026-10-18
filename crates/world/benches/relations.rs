use std::{hint::black_box, num::NonZeroU32};

use base::UUID;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use world::{
    ApplyOutcome::Introduced,
    FieldUpdate,
    Kind::Object,
    ObjectObservation, ObjectPatch,
    ParentUpdate::{Local, Root},
    Region, World,
};

use super::support::{PER_REGION, SCENE_SIZES, limits, size_name, source};

#[derive(Clone, Copy)]
struct RootCase {
    region: Region,
    id: UUID,
    child_count: usize,
}

pub(super) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/relations");
    for &count in SCENE_SIZES {
        let region_count = count.div_ceil(PER_REGION);
        group.throughput(Throughput::Elements((count - region_count) as u64));
        group.bench_with_input(BenchmarkId::new("parent_resolution", size_name(count)), &count, |b, &count| {
            let (mut verification, roots) = child_first_scene(count);
            for (index, root) in roots.iter().copied().enumerate() {
                let order = count as u64 + index as u64 + 1;
                let verified = verification.observe_object(root_observation(root, order)).expect("root arrival");
                let Introduced(entity) = verified.outcome else { panic!("expected root introduction") };
                assert_eq!(verification.children(entity).count(), root.child_count);
            }

            b.iter_batched(
                || child_first_scene(count),
                |(mut world, roots)| {
                    for (index, root) in roots.into_iter().enumerate() {
                        let order = count as u64 + index as u64 + 1;
                        let applied = world.observe_object(root_observation(root, order)).expect("root arrival");
                        let Introduced(_) = applied.outcome else { panic!("expected root introduction") };
                        let _ = black_box(applied);
                    }
                },
                BatchSize::LargeInput,
            );
        });
    }
    bench_deep_reparent(&mut group);
    group.finish();
}

fn child_first_scene(count: usize) -> (World, Vec<RootCase>) {
    let mut roots = Vec::with_capacity(count.div_ceil(PER_REGION));
    let mut world = World::new(limits());
    let mut first_child_id = 1_u128;
    let mut remaining = count;

    for region_index in 0..count.div_ceil(PER_REGION) {
        let region = Region::new(region_index as u64 + 1);
        assert!(matches!(world.adopt_region(region).expect("region adoption").outcome, world::RegionOutcome::Adopted { .. }));
        let entity_count = remaining.min(PER_REGION);
        let child_count = entity_count - 1;
        for local_index in 0..child_count {
            let local_id = NonZeroU32::new(local_index as u32 + 2).expect("child local ID is nonzero");
            let child = ObjectObservation {
                source: source(region, first_child_id as u64),
                id: UUID::from_u128(first_child_id),
                local_id,
                kind: Object,
                parent: Local(NonZeroU32::new(1).expect("root local ID is nonzero")),
                pose: None,
                linear_motion: None,
                angular_velocity: None,
                scale: None,
                attachment_point: FieldUpdate::Keep,
                shape: FieldUpdate::Keep,
                appearance: FieldUpdate::Keep,
                surface_appearance: None,
                render_materials: None,
            };
            first_child_id += 1;
            let _ = black_box(world.observe_object(child).expect("pending child introduction"));
        }

        roots.push(RootCase { region, id: UUID::from_u128(u128::MAX - region_index as u128), child_count });
        remaining -= entity_count;
    }

    (world, roots)
}

fn root_observation(root: RootCase, order: u64) -> ObjectObservation {
    ObjectObservation {
        source: source(root.region, order),
        id: root.id,
        local_id: NonZeroU32::new(1).expect("root local ID is nonzero"),
        kind: Object,
        parent: Root,
        pose: None,
        linear_motion: None,
        angular_velocity: None,
        scale: None,
        attachment_point: FieldUpdate::Keep,
        shape: FieldUpdate::Keep,
        appearance: FieldUpdate::Keep,
        surface_appearance: None,
        render_materials: None,
    }
}

fn bench_deep_reparent(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.throughput(Throughput::Elements(1));
    for &count in SCENE_SIZES {
        group.bench_with_input(BenchmarkId::new("deep_reparent", size_name(count)), &count, |b, &count| {
            let mut bounds = limits();
            bounds.max_entities_per_region = count;
            let mut world = World::new(bounds);
            let region = Region::new(1);
            let _ = world.adopt_region(region).expect("adopt deep region");
            for local in 1..=count as u32 {
                let parent = if local == 1 { Root } else { Local(NonZeroU32::new(local - 1).unwrap()) };
                let _ = world
                    .observe_object(ObjectObservation::new(
                        source(region, u64::from(local)),
                        UUID::from_u128(u128::from(local)),
                        NonZeroU32::new(local).unwrap(),
                        Object,
                        parent,
                    ))
                    .expect("deep child");
            }
            let mut order = count as u64 + 1;
            let mut alternate = false;
            b.iter(|| {
                let parent = count as u32 - if alternate { 1 } else { 2 };
                alternate = !alternate;
                let mut patch = ObjectPatch::new(source(region, order), NonZeroU32::new(count as u32).unwrap());
                order += 1;
                patch.parent = Some(Local(NonZeroU32::new(parent).unwrap()));
                let _ = black_box(world.patch_object(patch).expect("deep reparent"));
            });
        });
    }
}
