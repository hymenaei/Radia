use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use world::LocalAddress;

use super::support::{SCENE_SIZES, populated_scene, populated_scene_with_avatars, size_name};

pub(super) fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("world/queries");

    for &count in SCENE_SIZES {
        let scene = populated_scene(count, false, false);
        let row = scene.rows[count * 3 / 4];
        let address = LocalAddress::new(row.region, row.local_id);
        let entity = scene.world.entity(row.id).expect("benchmark object exists");

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("entity_by_uuid", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(scene.world.entity(row.id)));
        });
        group.bench_with_input(BenchmarkId::new("entity_by_local_id", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(scene.world.entity_at(address)));
        });
        group.bench_with_input(BenchmarkId::new("object", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(scene.world.object(entity)));
        });

        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("iterate_objects", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(scene.world.objects().fold(0usize, |count, _| count + 1)));
        });
        group.bench_with_input(BenchmarkId::new("iterate_regions", size_name(count)), &count, |b, _| {
            b.iter(|| {
                let total = scene
                    .regions
                    .iter()
                    .map(|region| scene.world.objects_in_region(*region).fold(0usize, |count, _| count + 1))
                    .sum::<usize>();
                black_box(total);
            });
        });

        let avatar_count = count.div_ceil(1_000);
        let avatars = populated_scene_with_avatars(count, avatar_count, false);
        assert_eq!(avatars.world.avatars().count(), avatar_count);
        group.throughput(Throughput::Elements(avatar_count as u64));
        group.bench_with_input(BenchmarkId::new("iterate_avatars", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(avatars.world.avatars().fold(0usize, |count, _| count + 1)));
        });
    }

    group.finish();
}
