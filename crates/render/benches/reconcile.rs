use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::support;

const OBJECTS: usize = 10_000;
const REGIONS: usize = 4;

pub(crate) fn bench(c: &mut Criterion) {
    let mut initial = c.benchmark_group("scene/reconcile_initial");
    initial.sample_size(10);
    initial.bench_function("first_full_view_batch/10k_4_regions", |b| {
        b.iter_batched(
            || support::scene_world(OBJECTS, REGIONS),
            |(world, _)| {
                support::first_full_view_batch(&world, OBJECTS);
                std::hint::black_box(world.object_count());
            },
            BatchSize::LargeInput,
        );
    });
    initial.finish();

    let (mut world, rows) = support::scene_world(OBJECTS, REGIONS);
    let (mut reconciler, mut view, mut now) = support::settle(&world, OBJECTS);
    let mut order = 2_000_000;
    let mut generation = 1;
    let mut incremental = c.benchmark_group("scene/reconcile_incremental");
    incremental.bench_function("pose_1_percent/10k_4_regions", |b| {
        b.iter(|| {
            support::update_one_percent(&mut world, &rows, &mut order, generation);
            support::first_publish(&mut reconciler, &mut view, &world, &mut now);
            generation += 1;
            std::hint::black_box(view.object_count());
        });
    });
    incremental.finish();

    let mut mesh_lod = c.benchmark_group("scene/mesh_lod_lookup");
    mesh_lod.sample_size(10);
    for count in [16, 128, 1024] {
        mesh_lod.throughput(Throughput::Elements(count as u64));
        mesh_lod.bench_with_input(BenchmarkId::new("camera_change_distinct_assets", count), &count, |b, &count| {
            b.iter_batched(
                || support::ready_distinct_mesh_scene(count),
                |(mut reconciler, mut view, world, mut now)| {
                    let work = support::request_distinct_mesh_refinement(&mut reconciler, &mut view, &world, &mut now);
                    std::hint::black_box((
                        work.requested.len(),
                        work.released.len(),
                        view.objects().filter(|object| object.mesh_lod().is_some()).count(),
                    ));
                },
                BatchSize::LargeInput,
            );
        });
    }
    mesh_lod.finish();
}
