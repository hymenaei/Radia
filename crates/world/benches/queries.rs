use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};
use math::{DVec3, Quat, Vec3};
use world::{
    Kind, LocalAddress, MAX_QUERY_STATE_MAINTENANCE_WORK, ObjectPatch, ParentUpdate::Local, Pose, QueryCompleteness, RegionInfo,
    ValueFilter, WorldQuery, WorldQueryBudget,
};

use super::support::{INITIAL_ORDER, SCENE_SIZES, populated_scene, populated_scene_with_avatars, size_name, source};

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
            b.iter(|| black_box(scene.world.objects().iter().fold(0usize, |count, _| count + 1)));
        });
        group.bench_with_input(BenchmarkId::new("scan_transforms", size_name(count)), &count, |b, _| {
            b.iter(|| {
                for object in scene.world.objects().iter() {
                    black_box((object.id(), object.transform(), object.parent()));
                }
            });
        });
        group.bench_with_input(BenchmarkId::new("scan_motion", size_name(count)), &count, |b, _| {
            b.iter(|| {
                for object in scene.world.objects().iter() {
                    black_box((object.id(), object.linear_motion(), object.angular_velocity()));
                }
            });
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
            b.iter(|| {
                for object in avatars.world.avatars() {
                    black_box(object.id());
                }
            });
        });
        group.bench_with_input(BenchmarkId::new("scan_avatars", size_name(count)), &count, |b, _| {
            b.iter(|| {
                for object in avatars.world.objects().iter().filter(|object| object.kind() == Kind::Avatar) {
                    black_box(object.id());
                }
            });
        });
        let query = WorldQuery::live().kind(ValueFilter::Is(Kind::Avatar));
        let budget = WorldQueryBudget { work: 128, results: 128 };
        let page = avatars.world.query(query, None, budget).expect("bounded avatar query");
        assert_eq!(page.completeness, QueryCompleteness::Exact);
        assert_eq!(page.items.len(), avatar_count);
        group.bench_with_input(BenchmarkId::new("query_avatars", size_name(count)), &count, |b, _| {
            b.iter(|| black_box(avatars.world.query(query, None, budget).expect("bounded avatar query")));
        });
    }

    group.finish();
    bench_spatial_queries(c);
}

fn bench_spatial_queries(c: &mut Criterion) {
    let mut scene = populated_scene(10_000, false, false);
    let region = scene.regions[0];
    let _ = scene.world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region origin");
    let mut order = INITIAL_ORDER + 100_000;
    for row in &scene.rows {
        let index = row.local_id.get() - 1;
        let position = Vec3::new((index % 100 * 2 + 1) as f32, (index / 100 * 2 + 1) as f32, 1.0);
        let mut patch = ObjectPatch::new(source(region, order), row.local_id);
        order += 1;
        patch.pose = Some(Pose { position: Some(position), rotation: Some(Quat::IDENTITY) });
        let _ = scene.world.patch_object(patch).expect("grid position");
    }
    while scene.world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("repair grid").has_dirty_pressure {}
    let query = WorldQuery::live().within(DVec3::new(101.0, 101.0, 1.0), 0.25).expect("spatial query");
    let budget = WorldQueryBudget { work: 4096, results: 64 };
    let page = scene.world.query(query, None, budget).expect("clean spatial query");
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.len(), 1);
    c.bench_function("world/spatial_query/clean/10k", |b| {
        b.iter(|| black_box(scene.world.query(query, None, budget).expect("clean spatial query")));
    });

    let root = scene.rows[5050];
    for (offset, row) in scene.rows[5051..5054].iter().enumerate() {
        let mut patch = ObjectPatch::new(source(region, order), row.local_id);
        order += 1;
        patch.parent = Some(Local(root.local_id));
        patch.pose = Some(Pose { position: Some(Vec3::X * (offset + 1) as f32), rotation: Some(Quat::IDENTITY) });
        let _ = scene.world.patch_object(patch).expect("linkset child");
    }
    while scene.world.maintain_query_state(MAX_QUERY_STATE_MAINTENANCE_WORK).expect("repair linkset").has_dirty_pressure {}
    let mut patch = ObjectPatch::new(source(region, order), root.local_id);
    patch.pose = Some(Pose { position: Some(Vec3::new(400.0, 400.0, 1.0)), rotation: None });
    let _ = scene.world.patch_object(patch).expect("move root");
    let query = WorldQuery::live().within(DVec3::new(400.0, 400.0, 1.0), 4.0).expect("spatial query");
    let budget = WorldQueryBudget { work: 64, results: 8 };
    let page = scene.world.query(query, None, budget).expect("dirty spatial query");
    assert_eq!(page.completeness, QueryCompleteness::Exact);
    assert_eq!(page.items.len(), 4);
    c.bench_function("world/spatial_query/dirty_subtree/10k", |b| {
        b.iter(|| black_box(scene.world.query(query, None, budget).expect("dirty spatial query")));
    });
}
